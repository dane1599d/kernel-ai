//! A virtio-gpu 2D driver (VirtIO 1.2, 5.7), polled: display info and EDID for
//! the modes, one host resource as the screen, rectangle transfers and flushes,
//! and the hardware cursor.
//!
//! virtio-drivers 0.13 has a GPU device, but it flushes only the whole screen and
//! creates the cursor resource on every shape change; this driver keeps its queue
//! and transport and speaks the protocol itself. The back buffer is the screen
//! resource's backing: physically contiguous frames in the direct map, so drawing
//! needs no copy and `flush` sends only the two commands.

use crate::virtio::hal::KernelHal;
use crate::virtio::split::SplitTransport;
use gfxcore::Rect;
use gfxcore::edid::Mode;
use gfxcore::pattern::CURSOR;
use virtio_drivers::queue::VirtQueue;
use virtio_drivers::transport::{DeviceStatus, Transport};
use virtio_drivers::{BufferDirection, Hal, PAGE_SIZE};

const CONTROL_QUEUE: u16 = 0;
const CURSOR_QUEUE: u16 = 1;
/// One request at a time, two descriptors each.
const QUEUE_SIZE: usize = 4;

const FEATURE_EDID: u64 = 1 << 1;
const FEATURE_VERSION_1: u64 = 1 << 32;

const GET_DISPLAY_INFO: u32 = 0x0100;
const RESOURCE_CREATE_2D: u32 = 0x0101;
const SET_SCANOUT: u32 = 0x0103;
const RESOURCE_FLUSH: u32 = 0x0104;
const TRANSFER_TO_HOST_2D: u32 = 0x0105;
const RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const GET_EDID: u32 = 0x010A;
const UPDATE_CURSOR: u32 = 0x0300;
const MOVE_CURSOR: u32 = 0x0301;
const OK_NODATA: u32 = 0x1100;
const OK_DISPLAY_INFO: u32 = 0x1101;
const OK_EDID: u32 = 0x1104;

const B8G8R8A8_UNORM: u32 = 1;
const B8G8R8X8_UNORM: u32 = 2;

const SCANOUT: u32 = 0;
const SCREEN: u32 = 1;
const CURSOR_RESOURCE: u32 = 2;

/// The command page: control requests, cursor requests and responses.
const CONTROL_AT: usize = 0;
const CURSOR_AT: usize = 512;
const RESPONSE_AT: usize = 1024;
/// A control header (24 bytes), size and padding, then the 1024-byte EDID blob.
const EDID_RESPONSE: usize = 24 + 8 + 1024;
/// Header, then 16 scanouts of a rectangle, enabled and flags.
const DISPLAY_INFO_RESPONSE: usize = 24 + 16 * 24;

/// Frames from `KernelHal::dma_alloc`, in the direct map.
struct Frames {
    phys: u64,
    virt: *mut u8,
    pages: usize,
}

impl Frames {
    fn new(bytes: usize) -> Result<Self, &'static str> {
        let pages = bytes.div_ceil(PAGE_SIZE);
        let (phys, virt) = KernelHal::dma_alloc(pages, BufferDirection::Both);
        if phys == 0 {
            return Err("virtio-gpu: out of contiguous frames");
        }
        Ok(Self {
            phys,
            virt: virt.as_ptr(),
            pages,
        })
    }

    fn bytes(&mut self, at: usize, len: usize) -> &mut [u8] {
        assert!(at + len <= self.pages * PAGE_SIZE);
        // SAFETY: the frames are ours, mapped and `pages` long; the borrow of self
        // keeps any other slice of them from being used meanwhile.
        unsafe { core::slice::from_raw_parts_mut(self.virt.add(at), len) }
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        let virt = core::ptr::NonNull::new(self.virt).expect("direct-map address is not null");
        // SAFETY: allocated by dma_alloc with this many pages, and the device no
        // longer uses them: the Gpu's drop resets the device first.
        unsafe { KernelHal::dma_dealloc(self.phys, virt, self.pages) };
    }
}

/// Little-endian 32-bit words written into a request.
struct Words<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl Words<'_> {
    fn u32(mut self, value: u32) -> Self {
        self.out[self.len..self.len + 4].copy_from_slice(&value.to_le_bytes());
        self.len += 4;
        self
    }

    fn u64(self, value: u64) -> Self {
        self.u32(value as u32).u32((value >> 32) as u32)
    }

    fn rect(self, rect: Rect) -> Self {
        self.u32(rect.x)
            .u32(rect.y)
            .u32(rect.width)
            .u32(rect.height)
    }
}

/// A request starting with a control header for `command` (no fence, context 0).
fn request(out: &mut [u8], command: u32) -> Words<'_> {
    Words { out, len: 0 }
        .u32(command)
        .u32(0)
        .u64(0)
        .u32(0)
        .u32(0)
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

pub struct Gpu {
    transport: SplitTransport,
    control: VirtQueue<KernelHal, QUEUE_SIZE>,
    cursor_queue: VirtQueue<KernelHal, QUEUE_SIZE>,
    commands: Frames,
    edid: bool,
    mode: Mode,
    screen: Option<Frames>,
    cursor: Option<Frames>,
}

impl Gpu {
    /// Resets the device, negotiates VERSION_1 and EDID, and sets up both queues.
    pub fn new(mut transport: SplitTransport) -> Result<Self, &'static str> {
        transport.set_status(DeviceStatus::empty());
        transport.set_status(DeviceStatus::ACKNOWLEDGE | DeviceStatus::DRIVER);
        let offered = transport.read_device_features();
        if offered & FEATURE_VERSION_1 == 0 {
            return Err("virtio-gpu: the device is legacy-only");
        }
        let features = offered & (FEATURE_VERSION_1 | FEATURE_EDID);
        transport.write_driver_features(features);
        transport.set_status(
            DeviceStatus::ACKNOWLEDGE | DeviceStatus::DRIVER | DeviceStatus::FEATURES_OK,
        );
        if !transport.get_status().contains(DeviceStatus::FEATURES_OK) {
            return Err("virtio-gpu: the device refused the features");
        }
        let queue = |transport: &mut SplitTransport, index| {
            VirtQueue::new(transport, index, false, false)
                .map_err(|_| "virtio-gpu: cannot set up a queue")
        };
        let control = queue(&mut transport, CONTROL_QUEUE)?;
        let cursor_queue = queue(&mut transport, CURSOR_QUEUE)?;
        let commands = Frames::new(PAGE_SIZE)?;
        transport.finish_init();
        Ok(Self {
            transport,
            control,
            cursor_queue,
            commands,
            edid: features & FEATURE_EDID != 0,
            mode: (0, 0),
            screen: None,
            cursor: None,
        })
    }

    /// Sends the `len`-byte control request at CONTROL_AT and waits for a
    /// `response`-byte answer at RESPONSE_AT; returns its type.
    fn control(&mut self, len: usize, response: usize) -> Result<u32, &'static str> {
        let page = self.commands.bytes(0, PAGE_SIZE);
        let (head, answer) = page.split_at_mut(RESPONSE_AT);
        self.control
            .add_notify_wait_pop(
                &[&head[CONTROL_AT..CONTROL_AT + len]],
                &mut [&mut answer[..response]],
                &mut self.transport,
            )
            .map_err(|_| "virtio-gpu: a control request failed")?;
        Ok(word(answer, 0))
    }

    /// Sends a control request that answers with no data.
    fn command(&mut self, build: impl FnOnce(&mut [u8]) -> usize) -> Result<(), &'static str> {
        let len = build(self.commands.bytes(CONTROL_AT, CURSOR_AT - CONTROL_AT));
        match self.control(len, 24)? {
            OK_NODATA => Ok(()),
            _ => Err("virtio-gpu: the device refused a command"),
        }
    }

    /// The enabled size of scanout 0, if the host has one.
    pub fn display_info(&mut self) -> Result<Option<Mode>, &'static str> {
        request(self.commands.bytes(CONTROL_AT, 24), GET_DISPLAY_INFO);
        if self.control(24, DISPLAY_INFO_RESPONSE)? != OK_DISPLAY_INFO {
            return Err("virtio-gpu: no display info");
        }
        let info = self.commands.bytes(RESPONSE_AT + 24, 24);
        let (width, height, enabled) = (word(info, 8), word(info, 12), word(info, 16));
        Ok((enabled != 0 && width > 0 && height > 0).then_some((width, height)))
    }

    /// The modes scanout 0's EDID lists; empty without the EDID feature.
    pub fn edid_modes(&mut self) -> Result<alloc::vec::Vec<Mode>, &'static str> {
        if !self.edid {
            return Ok(alloc::vec::Vec::new());
        }
        let len = request(self.commands.bytes(CONTROL_AT, 32), GET_EDID)
            .u32(SCANOUT)
            .u32(0)
            .len;
        if self.control(len, EDID_RESPONSE)? != OK_EDID {
            return Err("virtio-gpu: no EDID");
        }
        let answer = self.commands.bytes(RESPONSE_AT, EDID_RESPONSE);
        let size = (word(answer, 24) as usize).min(1024);
        Ok(gfxcore::edid::modes(&answer[32..32 + size]))
    }

    /// Creates a host resource of `width` by `height` backed by `frames`.
    fn create(
        &mut self,
        id: u32,
        format: u32,
        mode: Mode,
        frames: (u64, usize),
    ) -> Result<(), &'static str> {
        self.command(|out| {
            request(out, RESOURCE_CREATE_2D)
                .u32(id)
                .u32(format)
                .u32(mode.0)
                .u32(mode.1)
                .len
        })?;
        self.command(|out| {
            request(out, RESOURCE_ATTACH_BACKING)
                .u32(id)
                .u32(1)
                .u64(frames.0)
                .u32(frames.1 as u32)
                .u32(0)
                .len
        })
    }

    /// Makes a `mode` resource the screen and shows it on scanout 0. Returns the
    /// back buffer, zeroed: `mode.0` pixels per row, `mode.1` rows.
    pub fn set_mode(&mut self, mode: Mode) -> Result<*mut u32, &'static str> {
        if self.screen.is_some() {
            return Err("virtio-gpu: the mode is already set");
        }
        let bytes = mode.0 as usize * mode.1 as usize * 4;
        let frames = Frames::new(bytes)?;
        let (phys, base) = (frames.phys, frames.virt.cast::<u32>());
        self.screen = Some(frames);
        self.create(SCREEN, B8G8R8X8_UNORM, mode, (phys, bytes))?;
        self.command(|out| {
            request(out, SET_SCANOUT)
                .rect(Rect::screen(mode.0, mode.1))
                .u32(SCANOUT)
                .u32(SCREEN)
                .len
        })?;
        self.mode = mode;
        Ok(base)
    }

    fn transfer(&mut self, id: u32, rect: Rect, stride: u32) -> Result<(), &'static str> {
        let offset = (u64::from(rect.y) * u64::from(stride) + u64::from(rect.x)) * 4;
        self.command(|out| {
            request(out, TRANSFER_TO_HOST_2D)
                .rect(rect)
                .u64(offset)
                .u32(id)
                .u32(0)
                .len
        })
    }

    /// Copies `rect` of the back buffer to the host and shows it.
    pub fn flush(&mut self, rect: Rect) -> Result<(), &'static str> {
        let rect = rect.clamp(self.mode.0, self.mode.1);
        if rect.is_empty() {
            return Ok(());
        }
        self.transfer(SCREEN, rect, self.mode.0)?;
        self.command(|out| {
            request(out, RESOURCE_FLUSH)
                .rect(rect)
                .u32(SCREEN)
                .u32(0)
                .len
        })
    }

    /// Sends a cursor request; the cursor queue answers with nothing.
    fn cursor_request(
        &mut self,
        command: u32,
        at: (u32, u32),
        hot: (u32, u32),
    ) -> Result<(), &'static str> {
        let resource = if command == UPDATE_CURSOR && self.cursor.is_some() {
            CURSOR_RESOURCE
        } else {
            0
        };
        let len = request(
            self.commands.bytes(CURSOR_AT, RESPONSE_AT - CURSOR_AT),
            command,
        )
        .u32(SCANOUT)
        .u32(at.0)
        .u32(at.1)
        .u32(0)
        .u32(resource)
        .u32(hot.0)
        .u32(hot.1)
        .u32(0)
        .len;
        let page = self.commands.bytes(0, RESPONSE_AT);
        self.cursor_queue
            .add_notify_wait_pop(
                &[&page[CURSOR_AT..CURSOR_AT + len]],
                &mut [],
                &mut self.transport,
            )
            .map(|_| ())
            .map_err(|_| "virtio-gpu: a cursor request failed")
    }

    /// Sets the cursor's 64x64 `0xAARRGGBB` image and hot spot and shows it at `at`.
    pub fn set_cursor(
        &mut self,
        image: impl Fn(u32, u32) -> u32,
        hot: (u32, u32),
        at: (u32, u32),
    ) -> Result<(), &'static str> {
        let bytes = (CURSOR * CURSOR * 4) as usize;
        if self.cursor.is_none() {
            let frames = Frames::new(bytes)?;
            let phys = frames.phys;
            self.cursor = Some(frames);
            self.create(
                CURSOR_RESOURCE,
                B8G8R8A8_UNORM,
                (CURSOR, CURSOR),
                (phys, bytes),
            )?;
        }
        let pixels = self.cursor.as_mut().expect("just set").bytes(0, bytes);
        for (index, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let index = index as u32;
            pixel.copy_from_slice(&image(index % CURSOR, index / CURSOR).to_le_bytes());
        }
        self.transfer(CURSOR_RESOURCE, Rect::screen(CURSOR, CURSOR), CURSOR)?;
        self.cursor_request(UPDATE_CURSOR, at, hot)
    }

    /// Moves the cursor's hot spot to `at` without changing its image.
    pub fn move_cursor(&mut self, at: (u32, u32)) -> Result<(), &'static str> {
        self.cursor_request(MOVE_CURSOR, at, (0, 0))
    }
}

impl Drop for Gpu {
    /// A reset stops the device using the queues and frames freed after it.
    fn drop(&mut self) {
        self.transport.set_status(DeviceStatus::empty());
    }
}
