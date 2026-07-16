//! Platform audio backend (cpal) providing an implementation of `rust_gbe::AudioPlayer`.
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample};

struct CpalPlayer {
    buffer: Arc<Mutex<Vec<(f32, f32)>>>,
    sample_rate: u32,
}

impl CpalPlayer {
    fn get() -> Option<(CpalPlayer, cpal::Stream)> {
        let device = cpal::default_host().default_output_device()?;

        let wanted_samplerate = 44100;
        let supported_configs = device.supported_output_configs().ok()?;
        // Prefer a stereo f32 config, but any stereo config works — the stream
        // builder below handles every cpal sample format.
        let mut supported_config = None;
        let mut fallback_config = None;
        for f in supported_configs {
            if f.channels() != 2 {
                continue;
            }
            let config = if f.min_sample_rate() <= wanted_samplerate && wanted_samplerate <= f.max_sample_rate() {
                f.with_sample_rate(wanted_samplerate)
            } else {
                f.with_max_sample_rate()
            };
            if config.sample_format() == cpal::SampleFormat::F32 {
                supported_config = Some(config);
                break;
            }
            if fallback_config.is_none() {
                fallback_config = Some(config);
            }
        }
        let selected_config = supported_config.or(fallback_config)?;
        let sample_format = selected_config.sample_format();
        let config: cpal::StreamConfig = selected_config.into();

        let err_fn = |err| eprintln!("An error occurred on the output audio stream: {}", err);
        let shared_buffer = Arc::new(Mutex::new(Vec::new()));
        let stream_buffer = shared_buffer.clone();
        let player = CpalPlayer { buffer: shared_buffer, sample_rate: config.sample_rate };

        let stream = match sample_format {
            cpal::SampleFormat::I8 => device.build_output_stream(&config, move |d:&mut [i8], _| cpal_thread(d,&stream_buffer), err_fn, None),
            cpal::SampleFormat::I16 => device.build_output_stream(&config, move |d:&mut [i16], _| cpal_thread(d,&stream_buffer), err_fn, None),
            cpal::SampleFormat::I32 => device.build_output_stream(&config, move |d:&mut [i32], _| cpal_thread(d,&stream_buffer), err_fn, None),
            cpal::SampleFormat::I64 => device.build_output_stream(&config, move |d:&mut [i64], _| cpal_thread(d,&stream_buffer), err_fn, None),
            cpal::SampleFormat::U8 => device.build_output_stream(&config, move |d:&mut [u8], _| cpal_thread(d,&stream_buffer), err_fn, None),
            cpal::SampleFormat::U16 => device.build_output_stream(&config, move |d:&mut [u16], _| cpal_thread(d,&stream_buffer), err_fn, None),
            cpal::SampleFormat::U32 => device.build_output_stream(&config, move |d:&mut [u32], _| cpal_thread(d,&stream_buffer), err_fn, None),
            cpal::SampleFormat::U64 => device.build_output_stream(&config, move |d:&mut [u64], _| cpal_thread(d,&stream_buffer), err_fn, None),
            cpal::SampleFormat::F32 => device.build_output_stream(&config, move |d:&mut [f32], _| cpal_thread(d,&stream_buffer), err_fn, None),
            cpal::SampleFormat::F64 => device.build_output_stream(&config, move |d:&mut [f64], _| cpal_thread(d,&stream_buffer), err_fn, None),
            // Non-exhaustive enum: an unknown future format degrades to
            // no-audio (caller falls back to the null player) instead of
            // aborting the process (release builds use panic = "abort").
            _ => return None,
        }.ok()?;
        stream.play().ok()?;
        Some((player, stream))
    }
}

fn cpal_thread<T: Sample + FromSample<f32>>(outbuffer: &mut [T], audio_buffer: &Arc<Mutex<Vec<(f32, f32)>>>) {
    let mut inbuffer = match audio_buffer.try_lock() {
        Ok(guard) => guard,
        Err(_) => {
            // Contention or poison: fill with silence and skip this callback.
            for sample in outbuffer.iter_mut() {
                *sample = T::from_sample(0.0f32);
            }
            return;
        }
    };
    let outlen = ::std::cmp::min(outbuffer.len()/2, inbuffer.len());
    for (i, (l,r)) in inbuffer.drain(..outlen).enumerate() {
        outbuffer[i*2] = T::from_sample(l);
        outbuffer[i*2+1] = T::from_sample(r);
    }
    // Zero any tail the queue couldn't fill; cpal buffers can hold stale
    // samples from a previous callback, which are audible on underrun.
    for sample in outbuffer[outlen * 2..].iter_mut() {
        *sample = T::from_sample(0.0f32);
    }
}

impl rust_gbe::AudioPlayer for CpalPlayer {
    fn play(&mut self, left: &[f32], right: &[f32]) {
        debug_assert_eq!(left.len(), right.len());
        let mut buf = match self.buffer.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let cap = self.sample_rate as usize;
        let incoming = left.len(); // each element is a stereo pair (l, r)
        if buf.len() + incoming > cap {
            let excess = buf.len() + incoming - cap;
            let drop_n = excess.min(buf.len());
            buf.drain(0..drop_n);
        }
        for (&l, &r) in left.iter().zip(right) {
            buf.push((l, r));
        }
    }
    fn samples_rate(&self) -> u32 { self.sample_rate }
    fn underflowed(&self) -> bool {
        match self.buffer.lock() {
            Ok(g) => g.is_empty(),
            Err(poisoned) => poisoned.into_inner().is_empty(),
        }
    }
}

/// Initialize audio output, returning a boxed `AudioPlayer` and the live stream.
pub fn init_audio() -> Option<(Box<dyn rust_gbe::AudioPlayer>, cpal::Stream)> {
    CpalPlayer::get().map(|(p,s)| (Box::new(p) as Box<dyn rust_gbe::AudioPlayer>, s))
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

    #[test]
    fn cpal_thread_zeroes_unfilled_tail() {
        let buffer = Arc::new(Mutex::new(vec![(0.25f32, -0.25f32)]));
        let mut out = [1.0f32; 8];
        cpal_thread(&mut out, &buffer);
        assert_eq!(&out[..2], &[0.25, -0.25]);
        assert!(
            out[2..].iter().all(|&s| s == 0.0),
            "tail must be silence, got {:?}",
            out
        );
    }

    #[test]
    fn cpal_thread_empty_queue_outputs_silence() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let mut out = [0.5f32; 4];
        cpal_thread(&mut out, &buffer);
        assert!(out.iter().all(|&s| s == 0.0), "got {:?}", out);
    }

    #[test]
    fn null_player_reports_underflow_and_discards() {
        let mut p = null_player();
        p.play(&[0.1, 0.2], &[0.3, 0.4]);
        assert!(p.underflowed());
        assert_eq!(p.samples_rate(), 44100);
    }
}
