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

        let wanted_samplerate = cpal::SampleRate(44100);
        let supported_configs = device.supported_output_configs().ok()?;
        let mut supported_config = None;
        for f in supported_configs {
            if f.channels() == 2 && f.sample_format() == cpal::SampleFormat::F32 {
                if f.min_sample_rate() <= wanted_samplerate && wanted_samplerate <= f.max_sample_rate() {
                    supported_config = Some(f.with_sample_rate(wanted_samplerate));
                } else {
                    supported_config = Some(f.with_max_sample_rate());
                }
                break;
            }
        }
        let selected_config = supported_config?;
        let sample_format = selected_config.sample_format();
        let config: cpal::StreamConfig = selected_config.into();

        let err_fn = |err| eprintln!("An error occurred on the output audio stream: {}", err);
        let shared_buffer = Arc::new(Mutex::new(Vec::new()));
        let stream_buffer = shared_buffer.clone();
        let player = CpalPlayer { buffer: shared_buffer, sample_rate: config.sample_rate.0 };

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
            sf => panic!("Unsupported sample format {}", sf),
        }.ok()?;
        stream.play().ok()?;
        Some((player, stream))
    }
}

fn cpal_thread<T: Sample + FromSample<f32>>(outbuffer: &mut [T], audio_buffer: &Arc<Mutex<Vec<(f32, f32)>>>) {
    let mut inbuffer = audio_buffer.lock().unwrap();
    let outlen = ::std::cmp::min(outbuffer.len()/2, inbuffer.len());
    for (i, (l,r)) in inbuffer.drain(..outlen).enumerate() {
        outbuffer[i*2] = T::from_sample(l);
        outbuffer[i*2+1] = T::from_sample(r);
    }
}

impl rust_gbe::AudioPlayer for CpalPlayer {
    fn play(&mut self, left: &[f32], right: &[f32]) {
        debug_assert_eq!(left.len(), right.len());
        let mut buf = self.buffer.lock().unwrap();
        for (&l,&r) in left.iter().zip(right) {
            if buf.len() > self.sample_rate as usize { return; } // cap ~1s buffered
            buf.push((l,r));
        }
    }
    fn samples_rate(&self) -> u32 { self.sample_rate }
    fn underflowed(&self) -> bool { self.buffer.lock().unwrap().is_empty() }
}

/// Initialize audio output, returning a boxed `AudioPlayer` and the live stream.
pub fn init_audio() -> Option<(Box<dyn rust_gbe::AudioPlayer>, cpal::Stream)> {
    CpalPlayer::get().map(|(p,s)| (Box::new(p) as Box<dyn rust_gbe::AudioPlayer>, s))
}
