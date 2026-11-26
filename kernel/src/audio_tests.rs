//! `test=audio-virtio` (P6-S1, spike/audio-virtio, never merged): plays 60 s of
//! 48 kHz stereo through the virtio-sound device the PCI walk found, on QEMU's
//! `none` audio backend (which drains in real time), and checks for underruns.
//! It logs the buffer and period sizes, the underruns, and the CPU time per
//! second of audio, which are the spike's figures.

use crate::virtio::hal::KernelHal;
use crate::{hpet, log, threads};
use spike_p6_s1 as spike;
use spin::Mutex;
use virtio_drivers::device::sound::VirtIOSound;
use virtio_drivers::transport::pci::PciTransport;

/// Seconds of audio played.
const SECONDS: u32 = 60;
/// Longest the run may take: the backend plays in real time.
const LIMIT_NS: u64 = 75_000_000_000;

static SOUND: Mutex<Option<PciTransport>> = Mutex::new(None);

/// The PCI walk's virtio-sound, kept for the case.
pub fn attach(transport: Option<PciTransport>) {
    if let Some(transport) = transport {
        log!("audio-virtio: a virtio-sound device was found");
        *SOUND.lock() = Some(transport);
    }
}

pub fn check() -> Result<(), &'static str> {
    let transport = SOUND
        .lock()
        .take()
        .ok_or("audio-virtio: no virtio-sound device")?;
    let mut sound = VirtIOSound::<KernelHal, _>::new(transport)
        .map_err(|_| "audio-virtio: the device did not set up")?;
    log!(
        "audio-virtio: {} jacks, {} streams, {} channel maps",
        sound.jacks(),
        sound.streams(),
        sound.chmaps()
    );
    log!(
        "audio-virtio: buffer {} bytes, period {} bytes ({} frames, {} ms), {} periods in flight",
        spike::BUFFER_BYTES,
        spike::PERIOD_BYTES,
        spike::PERIOD_BYTES / spike::FRAME_BYTES,
        spike::PERIOD_BYTES / spike::FRAME_BYTES * 1000 / spike::RATE,
        spike::PERIODS
    );
    let stats = spike::play(&mut sound, SECONDS, &hpet::nanos, &|| threads::sleep_ms(2)).map_err(
        |error| {
            log!("audio-virtio: play failed: {error:?}");
            "audio-virtio: play failed"
        },
    )?;
    log!(
        "audio-virtio: {} periods in {} ms of wall time, {} underruns, at most {} periods in flight",
        stats.periods,
        stats.elapsed_ns / 1_000_000,
        stats.underruns,
        stats.max_in_flight
    );
    // Microseconds of work per second of audio, and the longest single stretch.
    log!(
        "audio-virtio: {} us of CPU per second of audio ({} us in all), longest stretch {} us",
        stats.busy_ns / 1000 / u64::from(SECONDS),
        stats.busy_ns / 1000,
        stats.longest_busy_ns / 1000
    );
    if stats.underruns != 0 {
        return Err("audio-virtio: an underrun");
    }
    if stats.elapsed_ns > LIMIT_NS {
        return Err("audio-virtio: the run took far longer than the audio");
    }
    log!(
        "audio-virtio: {} periods played with no underrun",
        stats.periods
    );
    Ok(())
}
