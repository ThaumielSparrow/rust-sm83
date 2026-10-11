//! Platform audio backend (cpal) providing an implementation of `rust_gbe::AudioPlayer`.
//!
//! Samples flow emulator thread → lock-free SPSC ring (`rtrb`) → cpal callback,
//! so the real-time callback never blocks or contends on a lock. The emulator
//! paces itself against the ring's fill level (see `AudioClock`).
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample};

/// Ring capacity and the fill level the emulator steers toward, in
/// milliseconds of audio. The APU delivers ~12 ms chunks, so the target
/// leaves room for one chunk of jitter on each side.
const RING_MS: u32 = 150;
const TARGET_MS: u32 = 40;

/// Shared view of the audio queue for frame pacing, plus the stream's
/// health. Cheap to clone; read from the emulator and GUI threads.
#[derive(Clone)]
pub struct AudioClock {
    queued: Arc<AtomicUsize>,
    failed: Arc<AtomicBool>,
    target: usize,
}

impl AudioClock {
    /// Queue fill relative to the target: 1.0 = on target, 0.0 = empty.
    pub fn fill_ratio(&self) -> f64 {
        self.queued.load(Ordering::Relaxed) as f64 / self.target as f64
    }

    /// True once the stream reported an error (typically the device was
    /// unplugged); the GUI then rebuilds the stream on the new default device.
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }
}

struct CpalPlayer {
    producer: rtrb::Producer<(f32, f32)>,
    capacity: usize,
    queued: Arc<AtomicUsize>,
    sample_rate: u32,
}

impl CpalPlayer {
    fn get() -> Option<(CpalPlayer, cpal::Stream, AudioClock)> {
        let device = cpal::default_host().default_output_device()?;

        // Rank every supported range: stereo first (mono is downmixed;
        // surround gets L/R in its first two channels), then a standard rate
        // (44.1 kHz, else 48 kHz, else the highest offered), then f32.
        // Device lists can start with odd fixed-rate ranges (WASAPI offers
        // 5512 Hz first), so never just take the first match.
        let mut best: Option<((bool, u8, std::cmp::Reverse<u32>, bool), cpal::SupportedStreamConfig)> = None;
        for f in device.supported_output_configs().ok()? {
            if f.channels() == 0 {
                continue;
            }
            let (rate_rank, config) = [44_100, 48_000]
                .iter()
                .enumerate()
                .find(|&(_, &r)| f.min_sample_rate() <= r && r <= f.max_sample_rate())
                .map(|(i, &r)| (i as u8, f.with_sample_rate(r)))
                .unwrap_or_else(|| (2, f.with_max_sample_rate()));
            let rank = (
                config.channels() != 2,
                rate_rank,
                std::cmp::Reverse(config.sample_rate()),
                config.sample_format() != cpal::SampleFormat::F32,
            );
            if best.as_ref().is_none_or(|(r, _)| rank < *r) {
                best = Some((rank, config));
            }
        }
        let (_, selected_config) = best?;
        let sample_format = selected_config.sample_format();
        let config: cpal::StreamConfig = selected_config.into();
        let channels = config.channels as usize;
        let sample_rate = config.sample_rate;

        let capacity = (sample_rate * RING_MS / 1000) as usize;
        let (producer, mut consumer) = rtrb::RingBuffer::new(capacity);
        let queued = Arc::new(AtomicUsize::new(0));
        let failed = Arc::new(AtomicBool::new(false));
        let clock = AudioClock {
            queued: queued.clone(),
            failed: failed.clone(),
            target: (sample_rate * TARGET_MS / 1000) as usize,
        };

        let err_failed = failed.clone();
        let err_fn = move |err| {
            crate::log::warn(format!("An error occurred on the output audio stream: {}", err));
            err_failed.store(true, Ordering::Relaxed);
        };
        let cb_queued = queued.clone();
        macro_rules! build {
            ($t:ty) => {
                device.build_output_stream(
                    &config,
                    move |d: &mut [$t], _| {
                        cpal_thread(d, channels, &mut consumer);
                        cb_queued.store(consumer.slots(), Ordering::Relaxed);
                    },
                    err_fn,
                    None,
                )
            };
        }
        let stream = match sample_format {
            cpal::SampleFormat::I8 => build!(i8),
            cpal::SampleFormat::I16 => build!(i16),
            cpal::SampleFormat::I32 => build!(i32),
            cpal::SampleFormat::I64 => build!(i64),
            cpal::SampleFormat::U8 => build!(u8),
            cpal::SampleFormat::U16 => build!(u16),
            cpal::SampleFormat::U32 => build!(u32),
            cpal::SampleFormat::U64 => build!(u64),
            cpal::SampleFormat::F32 => build!(f32),
            cpal::SampleFormat::F64 => build!(f64),
            // Non-exhaustive enum: an unknown future format degrades to
            // no-audio (caller falls back to the null player).
            _ => return None,
        }
        .ok()?;
        stream.play().ok()?;
        let player = CpalPlayer { producer, capacity, queued, sample_rate };
        Some((player, stream, clock))
    }
}

/// Fill one cpal buffer of `channels`-interleaved frames from the ring.
/// Underrun is padded with silence (cpal buffers can hold stale samples from a
/// previous callback, which are audible).
fn cpal_thread<T: Sample + FromSample<f32>>(
    outbuffer: &mut [T],
    channels: usize,
    consumer: &mut rtrb::Consumer<(f32, f32)>,
) {
    for frame in outbuffer.chunks_mut(channels) {
        let (l, r) = consumer.pop().unwrap_or((0.0, 0.0));
        match frame {
            [mono] => *mono = T::from_sample((l + r) * 0.5),
            [fl, fr, rest @ ..] => {
                *fl = T::from_sample(l);
                *fr = T::from_sample(r);
                for s in rest {
                    *s = T::from_sample(0.0f32);
                }
            }
            [] => {}
        }
    }
}

impl rust_gbe::AudioPlayer for CpalPlayer {
    fn play(&mut self, left: &[f32], right: &[f32]) {
        debug_assert_eq!(left.len(), right.len());
        // A full ring means the emulator is running ahead of the device
        // (turbo, or a stall on the audio side); drop what doesn't fit.
        for (&l, &r) in left.iter().zip(right) {
            if self.producer.push((l, r)).is_err() {
                break;
            }
        }
        self.queued.store(self.capacity - self.producer.slots(), Ordering::Relaxed);
    }
    fn samples_rate(&self) -> u32 { self.sample_rate }
    fn underflowed(&self) -> bool {
        self.producer.slots() == self.capacity
    }
}

/// Initialize audio output, returning a boxed `AudioPlayer`, the live stream,
/// and the queue clock used for pacing.
pub fn init_audio() -> Option<(Box<dyn rust_gbe::AudioPlayer>, cpal::Stream, AudioClock)> {
    CpalPlayer::get().map(|(p, s, c)| (Box::new(p) as Box<dyn rust_gbe::AudioPlayer>, s, c))
}

/// Player that discards all samples. Installed when no output device is
/// available so the APU is still emulated — games poll NR52 / length counters,
/// and their behavior must not depend on the host having a sound card.
struct NullPlayer;

impl rust_gbe::AudioPlayer for NullPlayer {
    fn play(&mut self, _left: &[f32], _right: &[f32]) {}
    fn samples_rate(&self) -> u32 { 44100 }
    // Always "hungry" so the APU keeps mixing instead of clearing its buffers.
    fn underflowed(&self) -> bool { true }
}

pub fn null_player() -> Box<dyn rust_gbe::AudioPlayer> {
    Box::new(NullPlayer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(samples: &[(f32, f32)]) -> rtrb::Consumer<(f32, f32)> {
        let (mut p, c) = rtrb::RingBuffer::new(16);
        for &s in samples {
            p.push(s).unwrap();
        }
        c
    }

    #[test]
    fn cpal_thread_zeroes_unfilled_tail() {
        let mut c = ring(&[(0.25, -0.25)]);
        let mut out = [1.0f32; 8];
        cpal_thread(&mut out, 2, &mut c);
        assert_eq!(&out[..2], &[0.25, -0.25]);
        assert!(out[2..].iter().all(|&s| s == 0.0), "tail must be silence, got {:?}", out);
    }

    #[test]
    fn cpal_thread_empty_queue_outputs_silence() {
        let mut c = ring(&[]);
        let mut out = [0.5f32; 4];
        cpal_thread(&mut out, 2, &mut c);
        assert!(out.iter().all(|&s| s == 0.0), "got {:?}", out);
    }

    #[test]
    fn cpal_thread_downmixes_mono_and_pads_surround() {
        let mut c = ring(&[(0.5, 0.25), (1.0, 0.0)]);
        let mut mono = [9.0f32; 2];
        cpal_thread(&mut mono, 1, &mut c);
        assert_eq!(mono, [0.375, 0.5]);

        let mut c = ring(&[(0.5, 0.25)]);
        let mut surround = [9.0f32; 6];
        cpal_thread(&mut surround, 6, &mut c);
        assert_eq!(surround, [0.5, 0.25, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn null_player_reports_underflow_and_discards() {
        let mut p = null_player();
        p.play(&[0.1, 0.2], &[0.3, 0.4]);
        assert!(p.underflowed());
        assert_eq!(p.samples_rate(), 44100);
    }
}
