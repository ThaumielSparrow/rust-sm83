//! High-level emulator orchestration: device construction, run loop & events.
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError};
use std::time::{Duration, Instant};

use rust_gbe::device::Device;

// Global setting for additional periodic auto-save functionality
static AUTO_SAVE_ENABLED: bool = false;

pub enum GBEvent {
    KeyUp(rust_gbe::KeypadKey),
    KeyDown(rust_gbe::KeypadKey),
    SpeedUp,
    SpeedDown,
    SaveState(u8),
    LoadState(u8),
    UpdateTurbo(crate::config::TurboSetting),
}

pub fn construct_cpu_auto(filename: &str) -> Option<Box<Device>> {
    let rom_path = std::path::Path::new(filename);
    let save_state_path = rom_path.with_extension("state");
    let save_state_str = save_state_path.to_string_lossy().to_string();
    // Try CGB first, fallback to classic
    match Device::new_cgb(filename, false, Some(save_state_str.clone())) {
        Ok(cpu) => Some(Box::new(cpu)),
        Err(_) => match Device::new(filename, false, Some(save_state_str)) {
            Ok(cpu) => Some(Box::new(cpu)),
            Err(msg) => { eprintln!("{}", msg); None }
        }
    }
}

pub fn run_cpu(mut cpu: Box<Device>, sender: SyncSender<Vec<u8>>, receiver: Receiver<GBEvent>) {
    // limit_speed: when true we pace at 1x (approx 60 FPS / 16ms per frame)
    // when false we apply turbo/slowmo pacing based on turbo_setting
    let mut limit_speed = true;
    // Will be updated from GUI shortly after thread spawn; start with Double as fallback
    let mut turbo_setting = crate::config::TurboSetting::Double;
    let mut last_frame_instant = Instant::now();

    let base_waitticks = (4_194_304f64 / 1000.0 * 16.0).round() as u32; // ~16ms frame chunk
    let mut ticks = 0;
    let mut frame_count = 0;
    let mut last_ram_save_frame = 0;
    let mut ram_needs_save = false;

    'outer: loop {
        // Always execute at least one frame worth of cycles.
        let frame_target = base_waitticks;
        while ticks < frame_target {
            ticks += cpu.do_cycle();
            if cpu.check_and_reset_gpu_updated() {
                let data = cpu.get_gpu_data().to_vec();
                if let Err(TrySendError::Disconnected(..)) = sender.try_send(data) { break 'outer; }
            }
        }
        ticks -= frame_target;
        frame_count += 1;

        if cpu.check_and_reset_ram_updated() {
            if cpu.save_battery_ram_silent().is_ok() {}
            ram_needs_save = false; last_ram_save_frame = frame_count;
        }
        if AUTO_SAVE_ENABLED && ram_needs_save && (frame_count - last_ram_save_frame) > 180 {
            if cpu.save_battery_ram_silent().is_ok() { ram_needs_save = false; }
        }

        'recv: loop { match receiver.try_recv() { Ok(ev)=>match ev {
            GBEvent::KeyUp(k)=>cpu.keyup(k), GBEvent::KeyDown(k)=>cpu.keydown(k), GBEvent::SpeedUp=>limit_speed=false,
            GBEvent::SpeedDown=>{limit_speed=true; cpu.sync_audio();}, GBEvent::SaveState(s)=>{println!("Attempting to save state to slot {}...",s); if let Err(e)=cpu.save_state_slot(s){eprintln!("Failed to save state to slot {}: {}",s,e);} },
            GBEvent::LoadState(s)=>{println!("Attempting to load state from slot {}...",s); if let Err(e)=cpu.load_state_slot(s){eprintln!("Failed to load state from slot {}: {}",s,e);} },
            GBEvent::UpdateTurbo(ts)=>{ turbo_setting = ts; },
        }, Err(TryRecvError::Empty)=>break 'recv, Err(TryRecvError::Disconnected)=>break 'outer } }

        // Timing / pacing
        let target_frame_ms = if limit_speed {
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
            if frame_count % 120 == 0 { std::thread::yield_now(); }
        }
        last_frame_instant = Instant::now();
    }
}

// (Old periodic timer removed in favor of Instant-based frame pacing.)
