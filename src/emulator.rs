//! High-level emulator orchestration: device construction, run loop & events.
use std::collections::VecDeque;
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
    /// Go back to the state from just before the last LoadState.
    UndoLoad,
    SetColorCorrection(bool),
    /// MBC7 accelerometer input (x, y) in g.
    Tilt(f32, f32),
    UpdateTurbo(crate::config::TurboSetting),
    UpdateVolume(f32), // master volume 0.0-1.0
    SetPaused(bool),
    SetRewinding(bool),
    RewindStep,
    SetCheats(Vec<crate::cheats::PokeOp>),
    /// New audio output (the old device went away). `None` clock = null player.
    SetAudio(Box<dyn rust_gbe::AudioPlayer>, Option<crate::audio::AudioClock>),
    Shutdown,
}

/// One Game Boy frame: 154 lines x 456 dots at 4.194304 MHz, ~16.74 ms (59.73 Hz).
const FRAME_TICKS: u32 = 154 * 456;
const FRAME_SECS: f64 = FRAME_TICKS as f64 / 4_194_304.0;
/// Most the audio queue may stretch or shrink the frame period: +/-0.5%, an
/// inaudible pitch change that absorbs the drift between the emulated clock
/// and the sound card's.
const AUDIO_RATE_CONTROL: f64 = 0.005;
/// Below this fraction of its target the audio queue is about to underrun:
/// skip the sleep and emulate ahead to refill it.
const AUDIO_LOW_WATER: f64 = 0.5;

/// Frame-period multiplier from the audio queue's fill (1.0 = on target):
/// a fuller queue slows emulation slightly, an emptier one speeds it up.
fn audio_rate_factor(fill_ratio: f64) -> f64 {
    1.0 + AUDIO_RATE_CONTROL * (fill_ratio - 1.0).clamp(-1.0, 1.0)
}

pub enum GuiEvent {
    SaveStateSaved { slot: u8, preview: SaveStatePreview },
    SaveStateFailed { slot: u8 },
    /// `error` is None on success.
    LoadStateResult { slot: u8, error: Option<&'static str> },
    BatterySaveFailed,
    UndoLoadResult { ok: bool },
    /// Rumble cartridge motor turned on/off.
    Rumble(bool),
}

/// Number of emulated frames between background battery-RAM saves. At ~60 fps
/// this is roughly one second; the exit-flush path catches any pending dirty
/// state, so this is purely a write-rate throttle to avoid hammering the disk
/// when a game touches SRAM many times per second.
const RAM_SAVE_DEBOUNCE_FRAMES: u64 = 60;

/// Capture a rewind snapshot every N emulated frames (~30 Hz at every-2).
const REWIND_INTERVAL_FRAMES: u64 = 2;
/// Ring-buffer depth in snapshots (~30 s of history at every-2-frames).
/// Entries are lz4-compressed, so this costs ~10-25 MB rather than ~160 MB raw.
const REWIND_CAPACITY: usize = 900;
/// Step-back distance for a discrete RewindStep (~1 s = 30 snapshots).
const REWIND_STEP_FRAMES: usize = 30;
/// Host loop iterations between rewind playback steps. One snapshot is consumed
/// every DIVISOR * ~16ms; each snapshot spans REWIND_INTERVAL_FRAMES emulated
/// frames, so 2 here gives ~1x real-time backward playback.
const REWIND_PLAYBACK_DIVISOR: u64 = 2;

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

/// True on frames where a rewind snapshot should be captured. `interval == 0`
/// disables capture.
fn should_capture(frame_count: u64, interval: u64) -> bool {
    interval != 0 && frame_count.is_multiple_of(interval)
}

/// How many ring entries to drop from the back to step back `step` moments,
/// always leaving at least one entry (the oldest) in the ring.
fn rewind_pops(len: usize, step: usize) -> usize {
    step.min(len.saturating_sub(1))
}

/// Copy `frame` into a free reusable buffer and try to send it to the GUI.
/// `Ok(())` covers both "sent" and "all buffers busy / channel full → frame
/// dropped"; only `Err(())` means the channel disconnected (caller shuts down).
fn try_send_frame(
    frame: &[u8],
    frame_buffers: &mut [Arc<Vec<u8>>],
    next_fb: &mut usize,
    sender: &SyncSender<Arc<Vec<u8>>>,
    wake: &dyn Fn(),
) -> Result<(), ()> {
    for attempt in 0..frame_buffers.len() {
        let idx = (*next_fb + attempt) % frame_buffers.len();
        if let Some(buf_mut) = Arc::get_mut(&mut frame_buffers[idx]) {
            buf_mut.copy_from_slice(frame);
            match sender.try_send(frame_buffers[idx].clone()) {
                Ok(_) => {
                    *next_fb = (idx + 1) % frame_buffers.len();
                    wake();
                    return Ok(());
                }
                Err(TrySendError::Disconnected(..)) => return Err(()),
                Err(TrySendError::Full(_)) => return Ok(()), // drop frame if GUI busy
            }
        }
    }
    Ok(())
}

/// Apply the enabled GameShark pokes. Called once per emulated frame while
/// running forward (real carts hook vblank); skipped while paused/rewinding.
fn apply_cheats(cpu: &mut Device, pokes: &[crate::cheats::PokeOp]) {
    for p in pokes {
        match p.bank {
            Some(b) => cpu.cheat_write_wram_bank(b, p.addr, p.value),
            None => cpu.write_byte(p.addr, p.value),
        }
    }
}

/// Decompress a rewind ring entry and restore it. Returns true on success;
/// failures are logged and skipped (entries are process-local, so neither
/// decompression nor decode should fail in practice).
fn restore_compressed(cpu: &mut Device, payload: &[u8]) -> bool {
    match lz4_flex::decompress_size_prepended(payload) {
        Ok(snap) => match cpu.restore_rewind(&snap) {
            Ok(()) => true,
            Err(e) => {
                crate::log::warn(format!("rewind restore failed: {}", e));
                false
            }
        },
        Err(_) => {
            crate::log::warn("rewind decompress failed".to_string());
            false
        }
    }
}

/// Largest ROM we'll unpack from an archive (the biggest real carts are 8 MiB).
const MAX_ROM_SIZE: u64 = 16 * 1024 * 1024;

/// Read a ROM file, or the first .gb/.gbc entry of a .zip archive.
pub fn read_rom(path: &std::path::Path) -> Result<Vec<u8>, &'static str> {
    let is_zip = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    if !is_zip {
        return std::fs::read(path).map_err(|_| "Could not read ROM");
    }
    let file = std::fs::File::open(path).map_err(|_| "Could not read ROM")?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| "Not a valid zip archive")?;
    for i in 0..archive.len() {
        let Ok(entry) = archive.by_index(i) else { continue };
        let is_rom = std::path::Path::new(entry.name())
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("gb") || e.eq_ignore_ascii_case("gbc"));
        if !entry.is_file() || !is_rom {
            continue;
        }
        if entry.size() > MAX_ROM_SIZE {
            return Err("ROM in archive is too large");
        }
        let mut data = Vec::with_capacity(entry.size() as usize);
        use std::io::Read;
        entry
            .take(MAX_ROM_SIZE)
            .read_to_end(&mut data)
            .map_err(|_| "Could not unpack ROM from archive")?;
        return Ok(data);
    }
    Err("No .gb or .gbc file in the archive")
}

/// Build a device for `filename`, preferring CGB mode. An invalid header
/// checksum only produces a warning (it is common in ROM hacks and homebrew);
/// on success the second element carries that warning for the GUI to show.
pub fn construct_cpu_auto(
    filename: &str,
) -> Result<(Box<Device>, Option<&'static str>), &'static str> {
    let rom_path = std::path::Path::new(filename);
    let save_state_str = rom_path.with_extension("state").to_string_lossy().to_string();
    let rom = read_rom(rom_path)?;
    let build = |skip_checksum: bool| {
        // Try CGB first, fallback to classic
        Device::new_cgb_from_bytes(rom.clone(), filename, skip_checksum, Some(save_state_str.clone()))
            .or_else(|_| {
                Device::new_from_bytes(rom.clone(), filename, skip_checksum, Some(save_state_str.clone()))
            })
            .map(Box::new)
    };
    match build(false) {
        Ok(cpu) => Ok((cpu, None)),
        Err(rust_gbe::CHECKSUM_INVALID) => {
            let cpu = build(true)?;
            Ok((cpu, Some("ROM header checksum is invalid — loaded anyway")))
        }
        Err(msg) => {
            crate::log::warn(msg);
            Err(msg)
        }
    }
}

/// Emulator thread entry point. A panic in the core is caught so battery RAM
/// can still be flushed; the panic hook in main.rs has already logged it, and
/// dropping the channels tells the GUI the emulator is gone.
///
/// `wake` nudges the GUI's event loop, which sleeps until something happens:
/// it is called after each frame or `GuiEvent` is sent, and on exit so the
/// GUI notices the channels closing.
pub fn run_cpu(
    mut cpu: Box<Device>,
    sender: SyncSender<Arc<Vec<u8>>>,
    receiver: Receiver<GBEvent>,
    ui_sender: Sender<GuiEvent>,
    wake: Box<dyn Fn() + Send>,
    audio: Option<crate::audio::AudioClock>,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_loop(&mut cpu, sender, receiver, ui_sender, &*wake, audio)
    }));
    if result.is_err() {
        // Skip the auto-save snapshot: it could capture a broken state.
        let _ = cpu.save_battery_ram_silent();
    }
    wake();
}

// Runs the emulation core loop. Sends video frames through a bounded channel.
fn run_loop(
    cpu: &mut Device,
    sender: SyncSender<Arc<Vec<u8>>>,
    receiver: Receiver<GBEvent>,
    ui_sender: Sender<GuiEvent>,
    wake: &dyn Fn(),
    mut audio: Option<crate::audio::AudioClock>,
) {
    // limit_speed: when true we pace at 1x (one GB frame per ~16.74 ms)
    // when false we apply turbo/slowmo pacing based on turbo_setting
    let mut limit_speed = true;
    // Will be updated from GUI shortly after thread spawn; start with Double as fallback
    let mut turbo_setting = crate::config::TurboSetting::Double;
    // Deadline for the end of the current frame. Advanced by one period per
    // frame (not reset to "now"), so sleep overshoot doesn't accumulate.
    let mut next_deadline = Instant::now();
    let mut paused = false;

    let mut ticks = 0;
    let mut frame_count: u64 = 0;

    // Battery-RAM autosave throttle: mark dirty when the cart touches SRAM, but
    // only write to disk every RAM_SAVE_DEBOUNCE_FRAMES frames. Exit paths flush
    // any pending dirty state so nothing is lost on quit.
    let mut ram_dirty = false;
    let mut last_ram_save_frame: Option<u64> = None;
    // Battery-save failure latch: toast only on the ok→fail transition so a
    // persistently failing disk produces one toast, not one per second.
    let mut battery_save_ok = true;

    // Two reusable frame buffers; we only write to a buffer if it is uniquely held (strong_count==1).
    let frame_len = cpu.get_gpu_data().len();
    let mut frame_buffers = [
        Arc::new(vec![0u8; frame_len]),
        Arc::new(vec![0u8; frame_len]),
    ];
    let mut next_fb = 0usize;

    let mut ring: VecDeque<Vec<u8>> = VecDeque::with_capacity(REWIND_CAPACITY);
    let mut rewinding = false;
    let mut rewind_tick: u64 = 0;
    let mut cheats: Vec<crate::cheats::PokeOp> = Vec::new();
    // State from just before the last successful slot load ("undo load").
    let mut undo_load: Option<Vec<u8>> = None;
    let mut rumbling = false;

    'outer: loop {
        if rewinding {
            // Rewind playback: do NOT advance the CPU. Step one snapshot back
            // every REWIND_PLAYBACK_DIVISOR host iterations for ~1x backward.
            rewind_tick = rewind_tick.wrapping_add(1);
            if rewind_tick.is_multiple_of(REWIND_PLAYBACK_DIVISOR) {
                for _ in 0..rewind_pops(ring.len(), 1) {
                    ring.pop_back();
                }
                if let Some(payload) = ring.back()
                    && restore_compressed(cpu, payload)
                        && try_send_frame(
                            cpu.get_gpu_data(),
                            &mut frame_buffers,
                            &mut next_fb,
                            &sender,
                            wake,
                        )
                        .is_err()
                    {
                        let _ = cpu.flush_to_disk();
                        break 'outer;
                    }
            }
        } else {
            // Normal forward execution.
            // Exactly one GB frame per iteration (ticks carry over), so each
            // iteration publishes one frame at a steady phase.
            let frame_target = FRAME_TICKS;
            while !paused && ticks < frame_target {
                ticks += cpu.do_cycle();
                if cpu.check_and_reset_gpu_updated()
                    && try_send_frame(
                        cpu.get_gpu_data(),
                        &mut frame_buffers,
                        &mut next_fb,
                        &sender,
                        wake,
                    )
                    .is_err()
                {
                    let _ = cpu.flush_to_disk();
                    break 'outer;
                }
            }
            if !paused {
                ticks -= frame_target;
                frame_count += 1;
                apply_cheats(cpu, &cheats);
                // Games pulse the motor many times per frame to vary strength;
                // the GUI only gets on/off as of the frame boundary.
                if cpu.rumble() != rumbling {
                    rumbling = !rumbling;
                    let _ = ui_sender.send(GuiEvent::Rumble(rumbling));
                    wake();
                }

                if should_capture(frame_count, REWIND_INTERVAL_FRAMES) {
                    match cpu.snapshot_rewind_with(lz4_flex::compress_prepend_size) {
                        Ok(compressed) => {
                            if ring.len() == REWIND_CAPACITY {
                                ring.pop_front();
                            }
                            ring.push_back(compressed);
                        }
                        Err(e) => crate::log::warn(format!("rewind snapshot failed: {}", e)),
                    }
                }

                if cpu.check_and_reset_ram_updated() {
                    ram_dirty = true;
                }
                if should_save_ram(
                    ram_dirty,
                    last_ram_save_frame,
                    frame_count,
                    RAM_SAVE_DEBOUNCE_FRAMES,
                ) {
                    let save_result = cpu.save_battery_ram_silent();
                    if save_result.is_err() && battery_save_ok {
                        let _ = ui_sender.send(GuiEvent::BatterySaveFailed);
                        wake();
                    }
                    battery_save_ok = save_result.is_ok();
                    ram_dirty = false;
                    last_ram_save_frame = Some(frame_count);
                }
            } else {
                ticks = 0;
            }
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
                        let thumbnail = thumbnail.as_deref().map(Vec::as_slice);
                        match cpu.save_state_slot(slot, thumbnail) {
                            Ok(preview) => {
                                let _ = ui_sender.send(GuiEvent::SaveStateSaved { slot, preview });
                        wake();
                            }
                            Err(e) => {
                                crate::log::warn(format!("Failed to save state to slot {}: {}", slot, e));
                                let _ = ui_sender.send(GuiEvent::SaveStateFailed { slot });
                        wake();
                            }
                        }
                    }
                    GBEvent::LoadState(s) => {
                        let before = cpu.snapshot_rewind().ok();
                        let error = cpu.load_state_slot(s).err();
                        match error {
                            None => undo_load = before,
                            Some(e) => {
                                crate::log::warn(format!("Failed to load state from slot {}: {}", s, e))
                            }
                        }
                        let _ = ui_sender.send(GuiEvent::LoadStateResult { slot: s, error });
                        wake();
                        ring.clear();
                    }
                    GBEvent::SetColorCorrection(on) => cpu.set_cgb_color_correction(on),
                    GBEvent::Tilt(x, y) => cpu.set_tilt(x, y),
                    GBEvent::UndoLoad => {
                        let ok = undo_load.take().is_some_and(|snap| cpu.restore_rewind(&snap).is_ok());
                        if ok {
                            ring.clear();
                        }
                        let _ = ui_sender.send(GuiEvent::UndoLoadResult { ok });
                        wake();
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
                    GBEvent::SetRewinding(r) => {
                        rewinding = r;
                        rewind_tick = 0;
                        // `ticks` is meaningless while rewinding (the CPU isn't
                        // stepped); reset so forward play resumes on a clean frame.
                        ticks = 0;
                        if !r {
                            cpu.sync_audio();
                        }
                    }
                    GBEvent::RewindStep => {
                        for _ in 0..rewind_pops(ring.len(), REWIND_STEP_FRAMES) {
                            ring.pop_back();
                        }
                        if let Some(payload) = ring.back()
                            && restore_compressed(cpu, payload) {
                                // A disconnect here is caught next iteration in
                                // the 'recv loop, which breaks 'outer cleanly.
                                let _ = try_send_frame(
                                    cpu.get_gpu_data(),
                                    &mut frame_buffers,
                                    &mut next_fb,
                                    &sender,
                                    wake,
                                );
                            }
                    }
                    GBEvent::SetCheats(list) => cheats = list,
                    GBEvent::SetAudio(player, clock) => {
                        cpu.set_audio_player(player);
                        audio = clock;
                    }
                    GBEvent::Shutdown => {
                        if let Err(e) = cpu.flush_to_disk() {
                            crate::log::warn(format!("flush_to_disk failed: {}", e));
                        }
                        break 'outer;
                    }
                },
                Err(TryRecvError::Empty) => break 'recv,
                Err(TryRecvError::Disconnected) => {
                    if let Err(e) = cpu.flush_to_disk() {
                        crate::log::warn(format!("flush_to_disk failed: {}", e));
                    }
                    break 'outer;
                }
            }
        }

        // Timing / pacing. Rewind playback and pause also tick at the frame
        // rate (rewind steps are paced by REWIND_PLAYBACK_DIVISOR; pause just
        // keeps polling events).
        let realtime = rewinding || paused || limit_speed;
        let period_secs = if realtime {
            FRAME_SECS
        } else {
            match turbo_setting.multiplier() {
                Some(m) => FRAME_SECS / m as f64, // m<1 => slow motion, m>1 => faster
                None => 0.0,                      // uncapped
            }
        };
        // At 1x the audio device is the master clock.
        let fill = audio
            .as_ref()
            .filter(|_| limit_speed && !paused && !rewinding)
            .map(|a| a.fill_ratio());

        if period_secs > 0.0 {
            let period = Duration::from_secs_f64(period_secs * fill.map_or(1.0, audio_rate_factor));
            next_deadline += period;
            let now = Instant::now();
            if fill.is_some_and(|f| f < AUDIO_LOW_WATER) {
                // About to underrun: run the next frame right away.
                next_deadline = now;
            } else if next_deadline > now {
                std::thread::sleep(next_deadline - now);
            } else if now - next_deadline > period * 4 {
                // Fell far behind (stall, window drag): resync instead of
                // sprinting to catch up.
                next_deadline = now;
            }
        } else {
            next_deadline = Instant::now();
            // Uncapped: still yield occasionally to avoid starving other threads
            if frame_count.is_multiple_of(120) {
                std::thread::yield_now();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_rate_factor_is_bounded_and_steers_toward_target() {
        assert_eq!(audio_rate_factor(1.0), 1.0);
        assert!(audio_rate_factor(1.5) > 1.0, "full queue slows emulation");
        assert!(audio_rate_factor(0.5) < 1.0, "empty queue speeds it up");
        assert_eq!(audio_rate_factor(100.0), 1.0 + AUDIO_RATE_CONTROL);
        assert_eq!(audio_rate_factor(0.0), 1.0 - AUDIO_RATE_CONTROL);
    }

    #[test]
    fn frame_period_is_59_73_hz() {
        assert!((1.0 / FRAME_SECS - 59.7275).abs() < 0.001);
    }

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

    #[test]
    fn capture_cadence_every_two_frames() {
        assert!(should_capture(0, 2));
        assert!(!should_capture(1, 2));
        assert!(should_capture(2, 2));
        assert!(!should_capture(5, 2));
        assert!(should_capture(6, 2));
        assert!(!should_capture(10, 0), "interval 0 never captures");
    }

    #[test]
    fn roms_load_from_zip_archives() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("rgbe-zip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("game.zip");
        let rom = std::fs::read("test/cpu_instrs.gb").unwrap();
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            z.start_file("readme.txt", opts).unwrap();
            z.write_all(b"not a rom").unwrap();
            z.start_file("Game (USA).GB", opts).unwrap();
            z.write_all(&rom).unwrap();
            z.finish().unwrap();
        }
        assert_eq!(read_rom(&zip_path).unwrap(), rom);
        let (dev, _) = construct_cpu_auto(&zip_path.to_string_lossy()).unwrap();
        assert_eq!(dev.romname(), "CPU_INSTRS");

        let empty = dir.join("empty.zip");
        zip::ZipWriter::new(std::fs::File::create(&empty).unwrap()).finish().unwrap();
        assert_eq!(read_rom(&empty), Err("No .gb or .gbc file in the archive"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_cheats_writes_plain_and_banked_pokes() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x147] = 0x00;
        let mut dev = Device::new_from_buffer(rom, true, None).unwrap();
        let pokes = vec![
            crate::cheats::PokeOp { bank: None, addr: 0xC010, value: 0x11 },
            crate::cheats::PokeOp { bank: Some(2), addr: 0xD020, value: 0x22 },
        ];
        apply_cheats(&mut dev, &pokes);
        assert_eq!(dev.read_byte(0xC010), 0x11);
        // DMG device: banked poke degrades to a plain bus write.
        assert_eq!(dev.read_byte(0xD020), 0x22);
    }

    #[test]
    fn compressed_rewind_snapshot_restores() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x147] = 0x00; // MBC0
        let mut dev = Device::new_from_buffer(rom, true, None).unwrap();
        dev.write_byte(0xC000, 0xAB);

        let snap = dev.snapshot_rewind().unwrap();
        let compressed = lz4_flex::compress_prepend_size(&snap);
        // WRAM starts pseudo-random, so the gain is modest; it must not grow.
        assert!(
            compressed.len() < snap.len(),
            "GB state should compress: {} -> {}",
            snap.len(),
            compressed.len()
        );

        dev.write_byte(0xC000, 0xCD);
        assert!(restore_compressed(&mut dev, &compressed));
        assert_eq!(dev.read_byte(0xC000), 0xAB, "state restored through compression");
    }

    #[test]
    fn rewind_pops_clamp_to_keep_one() {
        assert_eq!(rewind_pops(0, 30), 0);
        assert_eq!(rewind_pops(1, 30), 0);
        assert_eq!(rewind_pops(10, 30), 9);
        assert_eq!(rewind_pops(100, 30), 30);
        assert_eq!(rewind_pops(100, 1), 1);
    }
}
