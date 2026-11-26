//! P6-S1: 48 kHz stereo through a virtio-sound output stream with no underrun.
//!
//! The stream's buffer is [`PERIODS`] periods of [`PERIOD_BYTES`] bytes (20 ms of
//! audio each). The player keeps every period in flight on the TX queue,
//! reaps a completed one and queues the next as soon as it sees it, and sleeps
//! between looks. An underrun is a completion after which nothing is left in
//! flight while audio remains to play: the device had run dry.

#![no_std]
extern crate alloc;

#[cfg(test)]
extern crate std;

use alloc::collections::VecDeque;
use alloc::vec;
use virtio_drivers::device::sound::{
    PcmFeatures, PcmFormat, PcmFormats, PcmRate, PcmRates, VirtIOSound,
};
use virtio_drivers::transport::Transport;
use virtio_drivers::{Error, Hal};

/// Frames per second.
pub const RATE: u32 = 48_000;
/// Bytes in a frame: 2 channels of signed 16-bit samples.
pub const FRAME_BYTES: u32 = 4;
/// A period: 960 frames, 20 ms.
pub const PERIOD_BYTES: u32 = 960 * FRAME_BYTES;
/// Periods in the stream's buffer, and in flight at once.
pub const PERIODS: u32 = 4;
/// The stream's buffer: 80 ms.
pub const BUFFER_BYTES: u32 = PERIODS * PERIOD_BYTES;
/// Periods in `seconds` of audio.
pub const fn periods_in(seconds: u32) -> u32 {
    seconds * 1000 / 20
}

/// Peak sample of the tone: a quarter of full scale.
const AMPLITUDE: i64 = 8192;

/// Stereo test tone: 440 Hz left, 660 Hz right, phase-continuous across fills.
/// No floating point: a 1,024-entry sine table from Bhaskara's approximation.
pub struct Tone {
    table: [i16; 1024],
    left: u32,
    right: u32,
}

impl Default for Tone {
    fn default() -> Self {
        Self::new()
    }
}

impl Tone {
    pub fn new() -> Self {
        let mut table = [0i16; 1024];
        for t in 0..512i64 {
            let q = t * (512 - t);
            let v = (16 * q * AMPLITUDE / (1_310_720 - 4 * q)) as i16;
            table[t as usize] = v;
            table[t as usize + 512] = -v;
        }
        Self {
            table,
            left: 0,
            right: 0,
        }
    }

    /// Fills `out` (a whole number of frames) with the next frames.
    pub fn fill(&mut self, out: &mut [u8]) {
        const LEFT_STEP: u32 = ((440u64 << 32) / RATE as u64) as u32;
        const RIGHT_STEP: u32 = ((660u64 << 32) / RATE as u64) as u32;
        let (frames, _) = out.as_chunks_mut::<{ FRAME_BYTES as usize }>();
        for frame in frames {
            let l = self.table[(self.left >> 22) as usize];
            let r = self.table[(self.right >> 22) as usize];
            frame[..2].copy_from_slice(&l.to_le_bytes());
            frame[2..].copy_from_slice(&r.to_le_bytes());
            self.left = self.left.wrapping_add(LEFT_STEP);
            self.right = self.right.wrapping_add(RIGHT_STEP);
        }
    }
}

/// Counts underruns and the fullest the queue got.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tracker {
    pub underruns: u32,
    pub max_in_flight: usize,
}

impl Tracker {
    /// A period was queued, with `in_flight` now queued.
    pub fn queued(&mut self, in_flight: usize) {
        self.max_in_flight = self.max_in_flight.max(in_flight);
    }

    /// A period completed, with `in_flight` still queued and audio still to
    /// play (`more`): nothing left in flight means the device ran dry.
    pub fn completed(&mut self, in_flight: usize, more: bool) {
        if more && in_flight == 0 {
            self.underruns += 1;
        }
    }
}

/// What a run measured. Times are in nanoseconds of the caller's clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub periods: u32,
    pub underruns: u32,
    pub max_in_flight: usize,
    /// Wall time from the first queued period to the last completion.
    pub elapsed_ns: u64,
    /// Time spent working (filling, queueing, reaping), not asleep.
    pub busy_ns: u64,
    /// Longest single stretch of work.
    pub longest_busy_ns: u64,
}

/// Plays `seconds` of the tone on the device's first output stream, sleeping
/// with `pause` between looks at the queue, and says what it measured.
pub fn play<H: Hal, T: Transport>(
    sound: &mut VirtIOSound<H, T>,
    seconds: u32,
    clock: &dyn Fn() -> u64,
    pause: &dyn Fn(),
) -> Result<Stats, Error> {
    let stream = *sound.output_streams()?.first().ok_or(Error::Unsupported)?;
    if !sound
        .rates_supported(stream)?
        .contains(PcmRates::RATE_48000)
        || !sound.formats_supported(stream)?.contains(PcmFormats::S16)
    {
        return Err(Error::Unsupported);
    }
    sound.pcm_set_params(
        stream,
        BUFFER_BYTES,
        PERIOD_BYTES,
        PcmFeatures::empty(),
        2,
        PcmFormat::S16,
        PcmRate::Rate48000,
    )?;
    sound.pcm_prepare(stream)?;
    sound.pcm_start(stream)?;

    let total = periods_in(seconds);
    let mut tone = Tone::new();
    let mut period = vec![0u8; PERIOD_BYTES as usize];
    let mut flying: VecDeque<u16> = VecDeque::new();
    let mut tracker = Tracker::default();
    let (mut sent, mut done) = (0u32, 0u32);
    let (mut busy, mut longest) = (0u64, 0u64);

    let start = clock();
    while done < total {
        let t = clock();
        while sent < total && (flying.len() as u32) < PERIODS {
            tone.fill(&mut period);
            flying.push_back(sound.pcm_xfer_nb(stream, &period)?);
            sent += 1;
            tracker.queued(flying.len());
        }
        while let Some(&token) = flying.front() {
            match sound.pcm_xfer_ok(token) {
                Ok(()) => {
                    flying.pop_front();
                    done += 1;
                    tracker.completed(flying.len(), sent < total);
                    if sent < total {
                        tone.fill(&mut period);
                        flying.push_back(sound.pcm_xfer_nb(stream, &period)?);
                        sent += 1;
                        tracker.queued(flying.len());
                    }
                }
                Err(Error::NotReady) => break,
                Err(error) => return Err(error),
            }
        }
        let worked = clock() - t;
        busy += worked;
        longest = longest.max(worked);
        if done < total {
            pause();
        }
    }
    let elapsed = clock() - start;
    sound.pcm_stop(stream)?;
    sound.pcm_release(stream)?;
    Ok(Stats {
        periods: done,
        underruns: tracker.underruns,
        max_in_flight: tracker.max_in_flight,
        elapsed_ns: elapsed,
        busy_ns: busy,
        longest_busy_ns: longest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(bytes: &[u8]) -> std::vec::Vec<(i16, i16)> {
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|f| {
                (
                    i16::from_le_bytes([f[0], f[1]]),
                    i16::from_le_bytes([f[2], f[3]]),
                )
            })
            .collect()
    }

    #[test]
    fn sixty_seconds_is_three_thousand_whole_periods() {
        assert_eq!(periods_in(60), 3000);
        assert_eq!(PERIOD_BYTES, 3840);
        assert_eq!(BUFFER_BYTES % PERIOD_BYTES, 0);
        assert_eq!(u64::from(PERIOD_BYTES / FRAME_BYTES) * 50, u64::from(RATE));
    }

    #[test]
    fn the_tone_stays_inside_its_amplitude_and_has_the_right_pitch() {
        let mut tone = Tone::new();
        let mut second = std::vec![0u8; (RATE * FRAME_BYTES) as usize];
        tone.fill(&mut second);
        let s = samples(&second);
        assert!(s.iter().all(|&(l, r)| l.abs() <= 8192 && r.abs() <= 8192));
        assert!(s.iter().any(|&(l, _)| l > 8000) && s.iter().any(|&(_, r)| r < -8000));
        let rises = |pick: fn(&(i16, i16)) -> i16| {
            s.windows(2)
                .filter(|w| pick(&w[0]) < 0 && pick(&w[1]) >= 0)
                .count()
        };
        assert!((439..=441).contains(&rises(|f| f.0)), "{}", rises(|f| f.0));
        assert!((659..=661).contains(&rises(|f| f.1)), "{}", rises(|f| f.1));
    }

    #[test]
    fn fills_are_phase_continuous() {
        let mut whole = Tone::new();
        let mut a = std::vec![0u8; 2 * PERIOD_BYTES as usize];
        whole.fill(&mut a);
        let mut parts = Tone::new();
        let mut b = std::vec![0u8; 2 * PERIOD_BYTES as usize];
        let (first, second) = b.split_at_mut(PERIOD_BYTES as usize);
        parts.fill(first);
        parts.fill(second);
        assert_eq!(a, b);
    }

    #[test]
    fn a_completion_that_leaves_nothing_queued_while_audio_remains_is_an_underrun() {
        let mut t = Tracker::default();
        t.queued(4);
        t.completed(3, true);
        assert_eq!(t.underruns, 0);
        t.completed(0, false);
        assert_eq!(t.underruns, 0, "the last period leaves nothing queued");
        t.completed(0, true);
        assert_eq!(
            t,
            Tracker {
                underruns: 1,
                max_in_flight: 4
            }
        );
    }
}
