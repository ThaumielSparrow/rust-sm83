//! High-level emulator orchestration: device construction, run loop & events.
use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rust_gbe::device::{Device, SaveStatePreview};

pub enum GBEvent {
    KeyUp(rust_gbe::KeypadKey),
    KeyDown(rust_gbe::KeypadKey),
    SpeedUp,
    SpeedDown,
    SaveState {
        slot: u8,
        thumbnail: Option<Arc<Vec<u8>>>,
    },
    LoadState(u8),
    UpdateTurbo(crate::config::TurboSetting),
    UpdateVolume(f32), // master volume 0.0-1.0
    SetPaused(bool),
    Shutdown,
}

pub enum GuiEvent {
    SaveStateSaved { slot: u8, preview: SaveStatePreview },
    SaveStateFailed { slot: u8 },
}

/// Number of emulated frames between background battery-RAM saves. At ~60 fps
/// this is roughly one second; the exit-flush path catches any pending dirty
/// state, so this is purely a write-rate throttle to avoid hammering the disk
/// when a game touches SRAM many times per second.
const RAM_SAVE_DEBOUNCE_FRAMES: u64 = 60;

/// Pure debounce predicate for battery-RAM autosave. Returns true when a disk
/// write should happen on this frame: only when dirty, and only once the
/// throttle interval has elapsed since the last save (or no save yet).
fn should_save_ram(
    dirty: bool,
    last_save_frame: Option<u64>,
    current_frame: u64,
    threshold: u64,
) -> bool {
    if !dirty {
        return false;
    }
    match last_save_frame {
        None => true,
        Some(last) => current_frame.saturating_sub(last) >= threshold,
    }
}

pub fn construct_cpu_auto(filename: &str) -> Option<(Box<Device>, bool)> {
    let rom_path = std::path::Path::new(filename);
    let save_state_path = rom_path.with_extension("state");
    let save_state_str = save_state_path.to_string_lossy().to_string();
    // Try CGB first, fallback to classic
    match Device::new_cgb(filename, false, Some(save_state_str.clone())) {
        Ok(cpu) => {
            let is_color = cpu.is_cgb_mode();
            Some((Box::new(cpu), is_color))
        }
        Err(_) => match Device::new(filename, false, Some(save_state_str)) {
            Ok(cpu) => {
                let is_color = cpu.is_cgb_mode();
                Some((Box::new(cpu), is_color))
            }
            Err(msg) => {
                eprintln!("{}", msg);
                None
            }
        },
    }
}

// Runs the emulation core loop. Sends video frames through a bounded channel.
pub fn run_cpu(
    mut cpu: Box<Device>,
    sender: SyncSender<Arc<Vec<u8>>>,
    receiver: Receiver<GBEvent>,
    ui_sender: Sender<GuiEvent>,
) {
    // limit_speed: when true we pace at 1x (approx 60 FPS / 16ms per frame)
    // when false we apply turbo/slowmo pacing based on turbo_setting
    let mut limit_speed = true;
    // Will be updated from GUI shortly after thread spawn; start with Double as fallback
    let mut turbo_setting = crate::config::TurboSetting::Double;
    let mut last_frame_instant = Instant::now();
    let mut paused = false;

    let base_waitticks = (4_194_304f64 / 1000.0 * 16.0).round() as u32; // ~16ms frame chunk
    let mut ticks = 0;
    let mut frame_count: u64 = 0;

    // Battery-RAM autosave throttle: mark dirty when the cart touches SRAM, but
    // only write to disk every RAM_SAVE_DEBOUNCE_FRAMES frames. Exit paths flush
    // any pending dirty state so nothing is lost on quit.
    let mut ram_dirty = false;
    let mut last_ram_save_frame: Option<u64> = None;

    // Two reusable frame buffers; we only write to a buffer if it is uniquely held (strong_count==1).
    let frame_len = cpu.get_gpu_data().len();
    let mut frame_buffers = [
        Arc::new(vec![0u8; frame_len]),
        Arc::new(vec![0u8; frame_len]),
    ];
    let mut next_fb = 0usize;

    'outer: loop {
        // Always execute at least one frame worth of cycles (unless paused).
        let frame_target = base_waitticks;
        while !paused && ticks < frame_target {
            ticks += cpu.do_cycle();
            if cpu.check_and_reset_gpu_updated() {
                // Try to find a free (uniquely owned) buffer to copy into.
                for attempt in 0..frame_buffers.len() {
                    let idx = (next_fb + attempt) % frame_buffers.len();
                    if let Some(buf_mut) = Arc::get_mut(&mut frame_buffers[idx]) {
                        // Safe to mutate this buffer: no other references.
                        let src = cpu.get_gpu_data();
                        buf_mut.copy_from_slice(src);
                        match sender.try_send(frame_buffers[idx].clone()) {
                            Ok(_) => {
                                next_fb = (idx + 1) % frame_buffers.len();
                            }
                            Err(TrySendError::Disconnected(..)) => {
                                if let Err(e) = cpu.flush_to_disk() {
                                    eprintln!("flush_to_disk failed: {}", e);
                                }
                                break 'outer;
                            }
                            Err(TrySendError::Full(_)) => { /* Drop frame if receiver busy */ }
                        }
                        break;
                    }
                }
            }
        }
        if !paused {
            ticks -= frame_target;
            frame_count += 1;

            if cpu.check_and_reset_ram_updated() {
                ram_dirty = true;
            }
            if should_save_ram(ram_dirty, last_ram_save_frame, frame_count, RAM_SAVE_DEBOUNCE_FRAMES)
            {
                let _ = cpu.save_battery_ram_silent();
                ram_dirty = false;
                last_ram_save_frame = Some(frame_count);
            }
        } else {
            // While paused, ensure we don't busy-loop or overflow tick budgeting.
            ticks = 0;
        }

        'recv: loop {
            match receiver.try_recv() {
                Ok(ev) => match ev {
                    GBEvent::KeyUp(k) => cpu.keyup(k),
                    GBEvent::KeyDown(k) => cpu.keydown(k),
                    GBEvent::SpeedUp => limit_speed = false,
                    GBEvent::SpeedDown => {
                        limit_speed = true;
                        cpu.sync_audio();
                    }
                    GBEvent::SaveState { slot, thumbnail } => {
                        println!("Attempting to save state to slot {}...", slot);
                        let thumbnail = thumbnail.as_deref().map(Vec::as_slice);
                        match cpu.save_state_slot(slot, thumbnail) {
                            Ok(preview) => {
                                let _ = ui_sender.send(GuiEvent::SaveStateSaved { slot, preview });
                            }
                            Err(e) => {
                                eprintln!("Failed to save state to slot {}: {}", slot, e);
                                let _ = ui_sender.send(GuiEvent::SaveStateFailed { slot });
                            }
                        }
                    }
                    GBEvent::LoadState(s) => {
                        println!("Attempting to load state from slot {}...", s);
                        if let Err(e) = cpu.load_state_slot(s) {
                            eprintln!("Failed to load state from slot {}: {}", s, e);
                        }
                    }
                    GBEvent::UpdateTurbo(ts) => {
                        turbo_setting = ts;
                    }
                    GBEvent::UpdateVolume(v) => {
                        cpu.set_master_volume(v);
                    }
                    GBEvent::SetPaused(p) => {
                        if p && !paused {
                            cpu.sync_audio();
                        }
                        paused = p;
                    }
                    GBEvent::Shutdown => {
                        if let Err(e) = cpu.flush_to_disk() {
                            eprintln!("flush_to_disk failed: {}", e);
                        }
                        break 'outer;
                    }
                },
                Err(TryRecvError::Empty) => break 'recv,
                Err(TryRecvError::Disconnected) => {
                    if let Err(e) = cpu.flush_to_disk() {
                        eprintln!("flush_to_disk failed: {}", e);
                    }
                    break 'outer;
                }
            }
        }

        // Timing / pacing
        let target_frame_ms = if paused {
            16.0 // keep checking events at ~60 Hz while paused
        } else if limit_speed {
            16.0 // baseline ~60 FPS
        } else {
            match turbo_setting.multiplier() {
                Some(m) => 16.0 / m, // m<1 => slow motion (>16ms), m>1 => faster (<16ms)
                None => 0.0,         // uncapped
            }
        };

        if target_frame_ms > 0.0 {
            // Sleep to maintain target frame duration relative to last frame start
            let elapsed = last_frame_instant.elapsed();
            let target = Duration::from_secs_f64((target_frame_ms as f64) / 1000.0);
            if elapsed < target {
                std::thread::sleep(target - elapsed);
            }
        } else {
            // Uncapped: still yield occasionally to avoid starving other threads
            if frame_count % 120 == 0 {
                std::thread::yield_now();
            }
        }
        last_frame_instant = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_blocks_writes_until_threshold() {
        // First dirty frame with no prior save → save now.
        assert!(should_save_ram(true, None, 100, 60));
        // 59 frames since last save → still throttled.
        assert!(!should_save_ram(true, Some(100), 159, 60));
        // Exactly threshold reached → save.
        assert!(should_save_ram(true, Some(100), 160, 60));
        // Clean state never saves.
        assert!(!should_save_ram(false, Some(100), 1000, 60));
    }
}
