use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use cpal::Stream;
use glium::Surface;
use rust_gbe::device::{read_save_state_preview, SaveStatePreview};
use time::{Month, OffsetDateTime, UtcOffset};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::ModifiersState;
use winit::window::{Fullscreen, WindowId};

pub const EXITCODE_SUCCESS: i32 = 0;
pub const EXITCODE_CPULOADFAILS: i32 = 2;

#[derive(Default)]
pub struct RenderOptions {
    pub linear_interpolation: bool,
}

use crate::audio::init_audio;
use crate::config::{binding_value, config_path, Config, DmgPalettePreset, KeyBindings, TurboSetting};
use crate::emulator::{construct_cpu_auto, run_cpu, GBEvent, GuiEvent};
use crate::input::is_reserved_key_name;
use crate::palette::{apply_dmg_palette, palette_for_preset, DmgPalette};

struct SaveSlotUi {
    slot: u8,
    preview: Option<SaveStatePreview>,
    texture: Option<egui::TextureHandle>,
    saving: bool,
    save_failed: bool,
}

struct SaveSlotCache {
    slots: Vec<SaveSlotUi>,
}

impl SaveSlotCache {
    fn from_paths(paths: Vec<(u8, PathBuf)>) -> Self {
        let slots = paths
            .into_iter()
            .map(|(slot, path)| SaveSlotUi {
                slot,
                preview: read_save_state_preview(&path),
                texture: None,
                saving: false,
                save_failed: false,
            })
            .collect();
        SaveSlotCache { slots }
    }

    fn mark_saving(&mut self, slot: u8) {
        if let Some(slot) = self.slot_mut(slot) {
            slot.saving = true;
            slot.save_failed = false;
        }
    }

    fn mark_saved(&mut self, slot: u8, preview: SaveStatePreview) {
        if let Some(slot) = self.slot_mut(slot) {
            slot.preview = Some(preview);
            slot.texture = None;
            slot.saving = false;
            slot.save_failed = false;
        }
    }

    fn mark_failed(&mut self, slot: u8) {
        if let Some(slot) = self.slot_mut(slot) {
            slot.saving = false;
            slot.save_failed = true;
        }
    }

    fn slot_mut(&mut self, slot: u8) -> Option<&mut SaveSlotUi> {
        self.slots.iter_mut().find(|entry| entry.slot == slot)
    }
}

struct FpsMeter {
    times: VecDeque<Instant>,
    cap: usize,
}

impl FpsMeter {
    fn new() -> Self {
        Self { times: VecDeque::with_capacity(120), cap: 120 }
    }
    fn record(&mut self) {
        if self.times.len() == self.cap {
            self.times.pop_front();
        }
        self.times.push_back(Instant::now());
    }
    fn fps(&self) -> f32 {
        if self.times.len() < 2 {
            return 0.0;
        }
        let span = self
            .times
            .back()
            .unwrap()
            .duration_since(*self.times.front().unwrap())
            .as_secs_f32();
        if span <= 0.0 {
            0.0
        } else {
            (self.times.len() - 1) as f32 / span
        }
    }
}

// Unified state machine for ROM selection and emulator run to ensure a single EventLoop
enum RootPhase {
    Selecting {
        rom_path: String,
        browse_requested: bool,
    },
    Running {
        texture: glium::texture::texture2d::Texture2d,
        sender: mpsc::Sender<GBEvent>,
        receiver: Receiver<Arc<Vec<u8>>>,
        ui_receiver: Receiver<GuiEvent>,
        save_slots: SaveSlotCache,
        latest_frame: Option<Arc<Vec<u8>>>,
        renderoptions: RenderOptions,
        running: bool,
        keybindings: KeyBindings,
        capturing: Option<rust_gbe::KeypadKey>,
        _audio: Option<Stream>,
        show_keybindings_window: bool,
        // Timestamp of last Escape press. A single press will set this; a second press
        // within ESC_DOUBLE_PRESS_MS will trigger emulator exit. Kept here so the state
        // survives between key events.
        last_escape: Option<Instant>,
        turbo_toggle: bool,
        turbo_held: bool,
        turbo_setting: TurboSetting,
        volume: u8,
        rom_path: PathBuf,
        is_color: bool,
        emu_thread: Option<JoinHandle<()>>,
        modifiers: ModifiersState,
        paused: bool,
        pre_mute_volume: Option<u8>,
        fullscreen: bool,
        fps_overlay: bool,
        fps_meter: FpsMeter,
        dmg_palette_preset: DmgPalettePreset,
        dmg_palette_custom: [[u8; 3]; 4],
        // Scratch buffer reused across frames for host-side palette mapping in DMG mode.
        palette_scratch: Vec<u8>,
    },
}

/// Actions requested while the emulator phase is mutably borrowed, deferred until
/// after the borrow ends so they can call `&mut self` methods (stop_emulator, start_game_from_path).
enum PendingAction {
    Reset,
    LoadRom(PathBuf),
}

pub struct RootApp {
    window: Option<Arc<winit::window::Window>>,
    display: Option<glium::Display<glium::glutin::surface::WindowSurface>>,
    egui_glium: Option<egui_glium::EguiGlium>,
    phase: RootPhase,
    scale: u32,
    pending_rom: Option<PathBuf>,
    pending_action: Option<PendingAction>,
    pub exit_code: i32,
}

impl RootApp {
    pub fn new(scale: u32, pending_rom: Option<PathBuf>) -> Self {
        let default_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_string_lossy().to_string()))
            .unwrap_or_else(|| ".".to_string());
        let _cfg = Config::load(&config_path());
        RootApp {
            window: None,
            display: None,
            egui_glium: None,
            phase: RootPhase::Selecting {
                rom_path: default_dir,
                browse_requested: false,
            },
            scale,
            pending_rom,
            pending_action: None,
            exit_code: EXITCODE_SUCCESS,
        }
    }

    /// Drains any deferred actions set during a phase borrow. Called at the end of
    /// each window_event so reset / open-recent can call mutating methods on self.
    fn drain_pending_action(&mut self) {
        let action = self.pending_action.take();
        match action {
            Some(PendingAction::Reset) => {
                let path = match &self.phase {
                    RootPhase::Running { rom_path, .. } => Some(rom_path.clone()),
                    _ => None,
                };
                if let Some(p) = path {
                    self.stop_emulator();
                    self.start_game_from_path(p);
                }
            }
            Some(PendingAction::LoadRom(p)) => {
                if let RootPhase::Running { .. } = &self.phase {
                    self.stop_emulator();
                }
                self.start_game_from_path(p);
            }
            None => {}
        }
    }

    fn start_game_from_path(&mut self, rom_path: PathBuf) {
        // Always run in (CGB-capable) mode; attempt CGB first, fallback to classic if needed.
        let filename = rom_path.to_string_lossy().to_string();
        let (mut cpu, is_color) = match construct_cpu_auto(&filename) {
            Some(pair) => pair,
            None => {
                self.exit_code = EXITCODE_CPULOADFAILS;
                return;
            }
        };
        // Enable audio by default; if device fails, continue silently.
        let mut audio_stream = None;
        if let Some((player, s)) = init_audio() {
            cpu.enable_audio(player, true);
            audio_stream = Some(s);
        } else {
            warn("Audio disabled: no output device available");
        }
        let _ = cpu.romname();
        let save_slots = SaveSlotCache::from_paths(
            (1..=4)
                .map(|slot| (slot, cpu.save_state_slot_path(slot)))
                .collect(),
        );
        let (sender, recv_events) = mpsc::channel();
        let (frame_sender, frame_receiver) = mpsc::sync_channel(1);
        let (ui_sender, ui_receiver) = mpsc::channel();
        let frame_sender_clone = frame_sender.clone();
        let emu_thread = thread::spawn(move || run_cpu(cpu, frame_sender_clone, recv_events, ui_sender));
        if let Some(display) = &self.display {
            let texture = glium::texture::texture2d::Texture2d::empty_with_format(
                display,
                glium::texture::UncompressedFloatFormat::U8U8U8,
                glium::texture::MipmapsOption::NoMipmap,
                rust_gbe::SCREEN_W as u32,
                rust_gbe::SCREEN_H as u32,
            )
            .unwrap();
            let cfg = Config::load(&config_path());
            let initial_scale = cfg.scale;
            self.scale = initial_scale;

            // Push to recent ROMs.
            crate::config::update_config(|c| c.push_recent(&rom_path));

            self.phase = RootPhase::Running {
                texture,
                sender,
                receiver: frame_receiver,
                ui_receiver,
                save_slots,
                latest_frame: None,
                renderoptions: RenderOptions::default(),
                running: true,
                keybindings: cfg.keybindings.clone(),
                capturing: None,
                _audio: audio_stream,
                show_keybindings_window: false,
                last_escape: None,
                turbo_toggle: false,
                turbo_held: false,
                turbo_setting: cfg.turbo,
                volume: cfg.volume,
                rom_path,
                is_color,
                emu_thread: Some(emu_thread),
                modifiers: ModifiersState::empty(),
                paused: false,
                pre_mute_volume: None,
                fullscreen: cfg.fullscreen,
                fps_overlay: cfg.fps_overlay,
                fps_meter: FpsMeter::new(),
                dmg_palette_preset: cfg.dmg_palette_preset,
                dmg_palette_custom: cfg.dmg_palette_custom,
                palette_scratch: Vec::new(),
            };
            if let RootPhase::Running { sender, .. } = &self.phase {
                let _ = sender.send(GBEvent::UpdateTurbo(cfg.turbo));
                let _ = sender.send(GBEvent::UpdateVolume(perceptual_to_linear(cfg.volume)));
            }
            // Now that we've transitioned to Running, resize/configure window.
            if let Some(win) = &self.window {
                apply_window_mode(win, self.scale, cfg.fullscreen);
            }
        } else {
            self.exit_code = EXITCODE_CPULOADFAILS;
        }
    }

    /// Send Shutdown to the emulator worker and join its thread. Leaves `self.phase`
    /// in `Running` (with `emu_thread = None`) so the caller can immediately transition
    /// to a new phase (Selecting or another Running via `start_game_from_path`).
    fn stop_emulator(&mut self) {
        let handle = match &mut self.phase {
            RootPhase::Running { sender, emu_thread, .. } => {
                let _ = sender.send(GBEvent::Shutdown);
                emu_thread.take()
            }
            _ => None,
        };
        if let Some(h) = handle {
            let _ = h.join();
        }
    }
}

fn apply_window_mode(window: &winit::window::Window, scale: u32, fullscreen: bool) {
    if fullscreen {
        window.set_fullscreen(Some(Fullscreen::Borderless(None)));
    } else {
        window.set_fullscreen(None);
        set_window_size(window, scale);
    }
}

fn rom_path_is_supported(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|s| s.to_str()).map(|s| s.to_ascii_lowercase()),
        Some(ref ext) if ext == "gb" || ext == "gbc"
    )
}

impl ApplicationHandler for RootApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let (window, display) = glium::backend::glutin::SimpleWindowBuilder::new()
                .with_title("Game Boy Emulator")
                .with_inner_size(600, 220)
                .build(event_loop);
            let egui_glium =
                egui_glium::EguiGlium::new(egui::ViewportId::ROOT, &display, &window, &event_loop);
            self.egui_glium = Some(egui_glium);
            self.display = Some(display);
            self.window = Some(Arc::new(window));
            if let Some(w) = &self.window {
                w.request_redraw();
            }
            // If a ROM path was supplied on the command line, skip the file picker.
            if let Some(p) = self.pending_rom.take() {
                self.start_game_from_path(p);
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        use winit::event::ElementState::{Pressed, Released};
        use winit::keyboard::{Key, NamedKey};

        // Pass events to egui in all phases (menus in Running phase)
        if let Some(egui_glium) = &mut self.egui_glium {
            let resp = egui_glium.on_event(self.window.as_ref().unwrap(), &event);
            if resp.repaint {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            if resp.consumed {
                return;
            }
        }

        match (&mut self.phase, event) {
            (_, WindowEvent::CloseRequested) => {
                event_loop.exit();
            }
            (
                RootPhase::Selecting {
                    rom_path,
                    browse_requested,
                },
                WindowEvent::RedrawRequested,
            ) => {
                if *browse_requested {
                    *browse_requested = false;
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("Game Boy ROMs", &["gb", "gbc"])
                        .add_filter("All files", &["*"])
                        .set_directory(&rom_path)
                        .pick_file()
                    {
                        *rom_path = p.to_string_lossy().to_string();
                    }
                }
                let mut launch_path: Option<PathBuf> = None;
                let mut quit_requested = false;
                let recent_roms = Config::load(&config_path()).recent_roms;
                if let (Some(window), Some(display), Some(egui_glium)) =
                    (&self.window, &self.display, &mut self.egui_glium)
                {
                    egui_glium.run(window, |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            ui.heading("Game Boy Emulator");
                            ui.add_space(8.0);
                            ui.label("Select a ROM file to load:");
                            ui.horizontal(|ui| {
                                ui.label("ROM:");
                                ui.add(egui::TextEdit::singleline(rom_path).desired_width(340.0));
                                if ui.button("Browse").clicked() {
                                    *browse_requested = true;
                                }
                            });
                            ui.add_space(8.0);
                            if ui.button("Load ROM").clicked() {
                                let p = PathBuf::from(&*rom_path);
                                if p.is_file() {
                                    launch_path = Some(p);
                                }
                            }
                            if ui.button("Quit").clicked() {
                                quit_requested = true;
                            }
                            ui.add_space(6.0);
                            if rom_path.is_empty() {
                                ui.colored_label(
                                    egui::Color32::GRAY,
                                    "Enter a path to a .gb/.gbc file",
                                );
                            } else if !std::path::Path::new(&rom_path).exists() {
                                ui.colored_label(egui::Color32::RED, "File does not exist");
                            } else {
                                ui.colored_label(egui::Color32::GREEN, "Path OK");
                            }
                            if !recent_roms.is_empty() {
                                ui.add_space(10.0);
                                ui.label("Recent ROMs:");
                                for entry in &recent_roms {
                                    let label = std::path::Path::new(entry)
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| entry.clone());
                                    if ui
                                        .button(format!("{}  —  {}", label, entry))
                                        .on_hover_text(entry)
                                        .clicked()
                                    {
                                        let p = PathBuf::from(entry);
                                        if p.is_file() {
                                            launch_path = Some(p);
                                        }
                                    }
                                }
                            }
                            ui.add_space(8.0);
                            ui.label("Tip: drag a .gb/.gbc file onto this window to load it.");
                        });
                    });
                    // Paint after UI
                    let mut target = display.draw();
                    target.clear_color(0.1, 0.1, 0.1, 1.0);
                    egui_glium.paint(display, &mut target);
                    let _ = target.finish();
                }
                if let Some(p) = launch_path {
                    self.start_game_from_path(p);
                }
                if quit_requested {
                    self.exit_code = EXITCODE_CPULOADFAILS;
                    event_loop.exit();
                }
            }
            // Drag-and-drop ROM into the Selecting screen.
            (
                RootPhase::Selecting { rom_path, .. },
                WindowEvent::DroppedFile(path),
            ) => {
                if rom_path_is_supported(&path) && path.is_file() {
                    *rom_path = path.to_string_lossy().into_owned();
                    self.start_game_from_path(path);
                }
            }
            // Track modifier state so chords like Ctrl+R work.
            (
                RootPhase::Running { modifiers, .. },
                WindowEvent::ModifiersChanged(new_mods),
            ) => {
                *modifiers = new_mods.state();
            }
            // ESC double-press logic: require two presses within ESC_DOUBLE_PRESS_MS to exit.
            (
                RootPhase::Running {
                    sender,
                    save_slots,
                    latest_frame,
                    renderoptions,
                    running,
                    keybindings,
                    capturing,
                    show_keybindings_window,
                    last_escape,
                    turbo_toggle,
                    turbo_held,
                    volume,
                    modifiers,
                    paused,
                    pre_mute_volume,
                    fullscreen,
                    fps_overlay,
                    ..
                },
                WindowEvent::KeyboardInput {
                    event: keyevent, ..
                },
            ) => {
                let state = keyevent.state;
                let logical = keyevent.logical_key.clone();
                if let Some(kp) = *capturing {
                    // Capturing mode: ESC cancels, any other key assigns.
                    if let Key::Named(NamedKey::Escape) = logical.as_ref() {
                        *capturing = None;
                        return;
                    }
                    if matches!(state, winit::event::ElementState::Pressed) {
                        let value = key_to_string(&logical.as_ref());
                        match kp {
                            rust_gbe::KeypadKey::A => keybindings.a = value.clone(),
                            rust_gbe::KeypadKey::B => keybindings.b = value.clone(),
                            rust_gbe::KeypadKey::Start => keybindings.start = value.clone(),
                            rust_gbe::KeypadKey::Select => keybindings.select = value.clone(),
                            rust_gbe::KeypadKey::Up => keybindings.up = value.clone(),
                            rust_gbe::KeypadKey::Down => keybindings.down = value.clone(),
                            rust_gbe::KeypadKey::Left => keybindings.left = value.clone(),
                            rust_gbe::KeypadKey::Right => keybindings.right = value.clone(),
                        }
                        *capturing = None;
                        let bindings_clone = keybindings.clone();
                        crate::config::update_config(|c| c.keybindings = bindings_clone);
                    }
                    return; // don't treat as game input
                }
                // System action dispatch. Reset and ToggleFullscreen need post-match handling
                // (a method call or window mutation) once the &mut self.phase borrow is released.
                let mut request_reset = false;
                let mut apply_fs_now = false;
                if let Some(action) = crate::input::system_action_for(&logical.as_ref(), state, *modifiers) {
                    use crate::input::SystemAction;
                    match action {
                        SystemAction::SaveState(s) => {
                            request_save_state(sender, save_slots, s, latest_frame);
                        }
                        SystemAction::LoadState(s) => {
                            let _ = sender.send(GBEvent::LoadState(s));
                        }
                        SystemAction::TurboHold(press) => {
                            if press {
                                if !*turbo_toggle && !*turbo_held {
                                    let _ = sender.send(GBEvent::SpeedUp);
                                }
                                *turbo_held = true;
                            } else {
                                *turbo_held = false;
                                if !*turbo_toggle {
                                    let _ = sender.send(GBEvent::SpeedDown);
                                }
                            }
                        }
                        SystemAction::TurboToggle => {
                            *turbo_toggle = !*turbo_toggle;
                            if *turbo_toggle {
                                if !*turbo_held {
                                    let _ = sender.send(GBEvent::SpeedUp);
                                }
                            } else if !*turbo_held {
                                let _ = sender.send(GBEvent::SpeedDown);
                            }
                        }
                        SystemAction::ToggleInterpolation => {
                            renderoptions.linear_interpolation = !renderoptions.linear_interpolation;
                        }
                        SystemAction::TogglePause => {
                            *paused = !*paused;
                            let _ = sender.send(GBEvent::SetPaused(*paused));
                        }
                        SystemAction::Reset => {
                            request_reset = true;
                        }
                        SystemAction::ToggleFullscreen => {
                            *fullscreen = !*fullscreen;
                            let fs = *fullscreen;
                            crate::config::update_config(|c| c.fullscreen = fs);
                            apply_fs_now = true;
                        }
                        SystemAction::ToggleMute => {
                            if let Some(restored) = pre_mute_volume.take() {
                                *volume = restored;
                            } else {
                                *pre_mute_volume = Some(*volume);
                                *volume = 0;
                            }
                            let _ = sender.send(GBEvent::UpdateVolume(perceptual_to_linear(*volume)));
                            let v = *volume;
                            crate::config::update_config(|c| c.volume = v);
                        }
                        SystemAction::ToggleFpsOverlay => {
                            *fps_overlay = !*fps_overlay;
                            let on = *fps_overlay;
                            crate::config::update_config(|c| c.fps_overlay = on);
                        }
                    }
                    // Snapshot values we'll need outside the phase borrow.
                    let fs = *fullscreen;
                    let sc = self.scale;
                    if apply_fs_now {
                        if let Some(win) = &self.window {
                            apply_window_mode(win, sc, fs);
                        }
                    }
                    if request_reset {
                        self.pending_action = Some(PendingAction::Reset);
                    }
                    return;
                }
                match (state, logical.as_ref()) {
                    // Escape: if keybindings window open, close it immediately; else require double-press to exit.
                    (Pressed, Key::Named(NamedKey::Escape)) => {
                        if *show_keybindings_window {
                            *show_keybindings_window = false;
                            // Do not treat this as a potential exit press
                            *last_escape = None;
                        } else {
                            const ESC_DOUBLE_PRESS_MS: u128 = 500;
                            let now = Instant::now();
                            if let Some(prev) = last_escape {
                                if now.duration_since(*prev).as_millis() <= ESC_DOUBLE_PRESS_MS {
                                    // Second press within window -> exit
                                    *running = false;
                                    event_loop.exit();
                                    *last_escape = None;
                                } else {
                                    // Too slow; treat this as new first press
                                    *last_escape = Some(now);
                                }
                            } else {
                                // First press: record timestamp and do nothing else
                                *last_escape = Some(now);
                            }
                        }
                    }
                    (Pressed, wkey) => {
                        if let Some(k) = dynamic_winit_to_keypad(wkey, keybindings) {
                            let _ = sender.send(GBEvent::KeyDown(k));
                        }
                    }
                    (Released, wkey) => {
                        if let Some(k) = dynamic_winit_to_keypad(wkey, keybindings) {
                            let _ = sender.send(GBEvent::KeyUp(k));
                        }
                    }
                }
            }
            (
                RootPhase::Running {
                    sender,
                    texture,
                    receiver,
                    ui_receiver,
                    save_slots,
                    latest_frame,
                    renderoptions,
                    running,
                    keybindings,
                    capturing,
                    show_keybindings_window,
                    turbo_toggle,
                    turbo_setting,
                    volume,
                    paused,
                    fullscreen,
                    fps_overlay,
                    fps_meter,
                    dmg_palette_preset,
                    dmg_palette_custom,
                    is_color,
                    palette_scratch,
                    pre_mute_volume,
                    ..
                },
                WindowEvent::RedrawRequested,
            ) => {
                if !*running {
                    return;
                }
                drain_gui_events(ui_receiver, save_slots);
                // Deferred actions set inside the egui closure or below, applied after the borrow ends.
                let mut quit_requested = false;
                let mut reset_clicked = false;
                let mut open_recent: Option<PathBuf> = None;
                let mut new_scale: Option<u32> = None;
                let mut apply_fullscreen_now = false;
                let recent_roms = Config::load(&config_path()).recent_roms;
                if let (Some(display), Some(window), Some(egui_glium)) =
                    (&self.display, &self.window, &mut self.egui_glium)
                {
                    let mut menu_bar_height = 0.0;
                    let is_color_ro = *is_color;
                    let cur_scale = self.scale;
                    egui_glium.run(window, |ctx| {
                        let top_panel = egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
                            egui::MenuBar::new().ui(ui, |ui| {
                                ui.menu_button("File", |ui| {
                                    ui.menu_button("States", |ui| {
                                        show_states_menu(ui, sender, save_slots, latest_frame);
                                    });
                                    ui.add_enabled_ui(!recent_roms.is_empty(), |ui| {
                                        ui.menu_button("Open Recent", |ui| {
                                            for entry in &recent_roms {
                                                let label = std::path::Path::new(entry)
                                                    .file_name()
                                                    .map(|n| n.to_string_lossy().into_owned())
                                                    .unwrap_or_else(|| entry.clone());
                                                if ui.button(label).on_hover_text(entry).clicked() {
                                                    open_recent = Some(PathBuf::from(entry));
                                                    ui.close();
                                                }
                                            }
                                        });
                                    });
                                    ui.separator();
                                    if ui.button("Quit").clicked() {
                                        quit_requested = true;
                                        ui.close();
                                    }
                                });
                                ui.menu_button("Emulation", |ui| {
                                    if ui.checkbox(paused, "Pause (P)").changed() {
                                        let _ = sender.send(GBEvent::SetPaused(*paused));
                                    }
                                    if ui.button("Reset (Ctrl+R)").clicked() {
                                        reset_clicked = true;
                                        ui.close();
                                    }
                                    ui.separator();
                                    ui.menu_button("Turbo Speed", |ui| {
                                        for ts in TurboSetting::all() {
                                            let selected = *turbo_setting == *ts;
                                            if ui.radio(selected, ts.label()).clicked() {
                                                *turbo_setting = *ts;
                                                let ts_now = *ts;
                                                crate::config::update_config(|c| c.turbo = ts_now);
                                                let _ = sender.send(GBEvent::UpdateTurbo(*ts));
                                            }
                                        }
                                    });
                                    ui.checkbox(turbo_toggle, "Turbo Enabled (T)");
                                });
                                ui.menu_button("Display", |ui| {
                                    if ui.checkbox(fullscreen, "Fullscreen (F11)").changed() {
                                        apply_fullscreen_now = true;
                                        let fs = *fullscreen;
                                        crate::config::update_config(|c| c.fullscreen = fs);
                                    }
                                    ui.menu_button("Scale", |ui| {
                                        for s in 1..=4 {
                                            let selected = cur_scale == s;
                                            if ui.radio(selected, format!("{}x", s)).clicked() {
                                                new_scale = Some(s);
                                            }
                                        }
                                    });
                                    ui.separator();
                                    ui.checkbox(&mut renderoptions.linear_interpolation, "Linear interpolation (Y)");
                                    ui.add_enabled_ui(!is_color_ro, |ui| {
                                        ui.menu_button("DMG Palette", |ui| {
                                            for preset in DmgPalettePreset::all() {
                                                let selected = *dmg_palette_preset == *preset;
                                                if ui.radio(selected, preset.label()).clicked() {
                                                    *dmg_palette_preset = *preset;
                                                    let p = *preset;
                                                    crate::config::update_config(|c| c.dmg_palette_preset = p);
                                                }
                                            }
                                            ui.separator();
                                            ui.label("Custom shades (lightest → darkest):");
                                            let mut custom_changed = false;
                                            for i in 0..4 {
                                                let mut rgb = dmg_palette_custom[i];
                                                if ui.color_edit_button_srgb(&mut rgb).changed() {
                                                    dmg_palette_custom[i] = rgb;
                                                    custom_changed = true;
                                                }
                                            }
                                            if custom_changed {
                                                let pal = *dmg_palette_custom;
                                                crate::config::update_config(|c| c.dmg_palette_custom = pal);
                                            }
                                        });
                                    });
                                    ui.separator();
                                    if ui.checkbox(fps_overlay, "Show FPS (F9)").changed() {
                                        let on = *fps_overlay;
                                        crate::config::update_config(|c| c.fps_overlay = on);
                                    }
                                });
                                ui.menu_button("Settings", |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label("Volume");
                                        let mut v = *volume as i32;
                                        let slider = egui::Slider::new(&mut v, 0..=100).show_value(false);
                                        if ui.add(slider).changed() {
                                            *volume = v as u8;
                                            let lin = perceptual_to_linear(*volume);
                                            let _ = sender.send(GBEvent::UpdateVolume(lin));
                                            let vol_now = *volume;
                                            crate::config::update_config(|c| c.volume = vol_now);
                                        }
                                        ui.label(format!("{}%", *volume));
                                    });
                                    if ui.button("Mute (M)").clicked() {
                                        if let Some(restored) = pre_mute_volume.take() {
                                            *volume = restored;
                                        } else {
                                            *pre_mute_volume = Some(*volume);
                                            *volume = 0;
                                        }
                                        let _ = sender.send(GBEvent::UpdateVolume(perceptual_to_linear(*volume)));
                                        let v = *volume;
                                        crate::config::update_config(|c| c.volume = v);
                                    }
                                    ui.separator();
                                    if ui.button("Keybindings...").clicked() { *show_keybindings_window = true; }
                                });
                            });
                        });
                        menu_bar_height = top_panel.response.rect.height();

                        if *fps_overlay {
                            let fps = fps_meter.fps();
                            egui::Area::new(egui::Id::new("fps_overlay"))
                                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, menu_bar_height + 4.0))
                                .show(ctx, |ui| {
                                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                                        ui.label(format!("{:.0} FPS · {}", fps, turbo_setting.label()));
                                        if *paused {
                                            ui.colored_label(egui::Color32::LIGHT_YELLOW, "PAUSED");
                                        }
                                    });
                                });
                        }

                        if *show_keybindings_window {
                            egui::Window::new("Keybindings").open(show_keybindings_window).show(ctx, |ui| {
                                ui.label("Click a binding, then press a key (Esc to cancel capture). Reserved keys can't be used.");
                                let keys = [rust_gbe::KeypadKey::A, rust_gbe::KeypadKey::B, rust_gbe::KeypadKey::Start, rust_gbe::KeypadKey::Select,
                                    rust_gbe::KeypadKey::Up, rust_gbe::KeypadKey::Down, rust_gbe::KeypadKey::Left, rust_gbe::KeypadKey::Right];
                                for k in keys { ui.horizontal(|ui| {
                                    ui.label(match k { rust_gbe::KeypadKey::A=>"A", rust_gbe::KeypadKey::B=>"B", rust_gbe::KeypadKey::Start=>"Start", rust_gbe::KeypadKey::Select=>"Select", rust_gbe::KeypadKey::Up=>"Up", rust_gbe::KeypadKey::Down=>"Down", rust_gbe::KeypadKey::Left=>"Left", rust_gbe::KeypadKey::Right=>"Right" });
                                    let active = matches_capturing(*capturing, k);
                                    let val = binding_value(keybindings, k);
                                    let conflict = is_reserved_key_name(&val);
                                    let label = if active { "(press key)".to_string() } else { val.clone() };
                                    let mut button = egui::Button::new(label);
                                    if conflict {
                                        button = button.fill(egui::Color32::from_rgb(100,0,0));
                                    }
                                    if ui.add(button).clicked() {
                                        *capturing = Some(k);
                                    }
                                    if conflict {
                                        ui.colored_label(egui::Color32::RED, "Conflicts with system keybind");
                                    }
                                }); }
                                if capturing.is_some() && ui.button("Cancel Capture").clicked() { *capturing=None; }
                            });
                        }
                    });
                    if quit_requested {
                        *running = false;
                    }

                    // Draw game texture with offset for menu bar
                    use glium::Surface;
                    let mut target = display.draw();
                    let (target_w, target_h) = target.get_dimensions();

                    // Calculate menu bar height in pixels
                    let menu_bar_height_pixels =
                        (menu_bar_height * window.scale_factor() as f32) as u32;
                    let game_area_height = target_h.saturating_sub(menu_bar_height_pixels);

                    // Render game texture offset downward by menu bar height
                    if game_area_height > 0 {
                        let interpolation_type = if renderoptions.linear_interpolation {
                            glium::uniforms::MagnifySamplerFilter::Linear
                        } else {
                            glium::uniforms::MagnifySamplerFilter::Nearest
                        };
                        texture.as_surface().blit_whole_color_to(
                            &target,
                            &glium::BlitTarget {
                                left: 0,
                                bottom: game_area_height, // Position at bottom of available area
                                width: target_w as i32,
                                height: -(game_area_height as i32), // Negative height to flip Y
                            },
                            interpolation_type,
                        );
                    }

                    // Paint egui on top
                    egui_glium.paint(display, &mut target);
                    let _ = target.finish();

                    if quit_requested {
                        event_loop.exit();
                    }
                }
                // Drain any queued frames and upload (palette-mapped for DMG).
                let palette_now = palette_for_preset(*dmg_palette_preset, dmg_palette_custom);
                let needs_palette = !*is_color;
                loop {
                    match receiver.try_recv() {
                        Ok(data) => {
                            fps_meter.record();
                            upload_frame_with_palette(texture, &data, needs_palette, &palette_now, palette_scratch);
                            *latest_frame = Some(data);
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            *running = false;
                            event_loop.exit();
                            break;
                        }
                    }
                }
                // Apply deferred actions that need &mut self (scale change, fullscreen retoggle from menu).
                if let Some(s) = new_scale {
                    self.scale = s;
                    crate::config::update_config(|c| c.scale = s);
                    let fs = if let RootPhase::Running { fullscreen, .. } = &self.phase { *fullscreen } else { false };
                    if let Some(win) = &self.window {
                        apply_window_mode(win, s, fs);
                    }
                }
                if apply_fullscreen_now {
                    let fs = if let RootPhase::Running { fullscreen, .. } = &self.phase { *fullscreen } else { false };
                    if let Some(win) = &self.window {
                        apply_window_mode(win, self.scale, fs);
                    }
                }
                if reset_clicked {
                    self.pending_action = Some(PendingAction::Reset);
                }
                if let Some(p) = open_recent {
                    self.pending_action = Some(PendingAction::LoadRom(p));
                }
            }
            // Drag-and-drop a ROM onto the running emulator: tear down and load the new one.
            (
                RootPhase::Running { .. },
                WindowEvent::DroppedFile(path),
            ) => {
                if rom_path_is_supported(&path) && path.is_file() {
                    self.pending_action = Some(PendingAction::LoadRom(path));
                }
            }
            _ => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
        }
        // Process any deferred mutating actions queued during a phase borrow.
        self.drain_pending_action();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let RootPhase::Running {
            receiver,
            ui_receiver,
            save_slots,
            latest_frame,
            texture,
            running,
            fps_meter,
            is_color,
            dmg_palette_preset,
            dmg_palette_custom,
            palette_scratch,
            ..
        } = &mut self.phase
        {
            if !*running {
                return;
            }
            drain_gui_events(ui_receiver, save_slots);
            let palette_now = palette_for_preset(*dmg_palette_preset, dmg_palette_custom);
            let needs_palette = !*is_color;
            match receiver.try_recv() {
                Ok(data) => {
                    fps_meter.record();
                    upload_frame_with_palette(texture, &data, needs_palette, &palette_now, palette_scratch);
                    *latest_frame = Some(data);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    *running = false;
                    event_loop.exit();
                }
            }
        }
    }
}

fn request_save_state(
    sender: &mpsc::Sender<GBEvent>,
    save_slots: &mut SaveSlotCache,
    slot: u8,
    latest_frame: &Option<Arc<Vec<u8>>>,
) {
    save_slots.mark_saving(slot);
    let thumbnail = latest_frame.as_ref().map(Arc::clone);
    if sender.send(GBEvent::SaveState { slot, thumbnail }).is_err() {
        save_slots.mark_failed(slot);
    }
}

fn drain_gui_events(receiver: &Receiver<GuiEvent>, save_slots: &mut SaveSlotCache) {
    loop {
        match receiver.try_recv() {
            Ok(GuiEvent::SaveStateSaved { slot, preview }) => {
                save_slots.mark_saved(slot, preview);
            }
            Ok(GuiEvent::SaveStateFailed { slot }) => {
                save_slots.mark_failed(slot);
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => break,
        }
    }
}

fn show_states_menu(
    ui: &mut egui::Ui,
    sender: &mpsc::Sender<GBEvent>,
    save_slots: &mut SaveSlotCache,
    latest_frame: &Option<Arc<Vec<u8>>>,
) {
    ui.set_min_width(320.0);

    let mut load_requested = None;
    let mut save_requested = None;

    for slot in &mut save_slots.slots {
        let load_enabled = slot.preview.is_some() && !slot.saving;
        let status = slot_status_text(slot);
        let color = slot_status_color(slot);
        let slot_number = slot.slot;

        let row = ui.horizontal(|ui| {
            let slot_response = ui.add_sized(
                [58.0, 22.0],
                egui::Label::new(egui::RichText::new(format!("Slot {}", slot_number)).strong()),
            );
            let status_response = ui.add_sized(
                [138.0, 22.0],
                egui::Label::new(egui::RichText::new(status).color(color)),
            );

            let load_response = ui.add_enabled(
                load_enabled,
                egui::Button::new("Load").min_size(egui::vec2(52.0, 22.0)),
            );
            let save_response = ui.add(egui::Button::new("Save").min_size(egui::vec2(52.0, 22.0)));
            let hovered = slot_response.hovered()
                || status_response.hovered()
                || load_response.hovered()
                || save_response.hovered();
            let row_rect = slot_response
                .rect
                .union(status_response.rect)
                .union(load_response.rect)
                .union(save_response.rect);

            (
                load_response.clicked(),
                save_response.clicked(),
                hovered,
                row_rect,
            )
        });

        let (load_clicked, save_clicked, row_hovered, row_rect) = row.inner;
        if slot.preview.is_some() && (row_hovered || row.response.hovered()) {
            show_slot_preview_area(ui, slot, row_rect);
        }

        if load_clicked {
            load_requested = Some(slot_number);
        }
        if save_clicked {
            save_requested = Some(slot_number);
        }
    }

    if let Some(slot) = load_requested {
        let _ = sender.send(GBEvent::LoadState(slot));
        ui.close();
    }
    if let Some(slot) = save_requested {
        request_save_state(sender, save_slots, slot, latest_frame);
        ui.close();
    }
}

fn slot_status_text(slot: &SaveSlotUi) -> String {
    if slot.saving {
        "Saving...".to_string()
    } else if slot.save_failed {
        "Save failed".to_string()
    } else if let Some(preview) = &slot.preview {
        format_short_timestamp(preview.saved_at_unix_secs)
    } else {
        "Empty".to_string()
    }
}

fn slot_status_color(slot: &SaveSlotUi) -> egui::Color32 {
    if slot.save_failed {
        egui::Color32::from_rgb(210, 72, 64)
    } else if slot.preview.is_some() {
        egui::Color32::from_rgb(190, 220, 190)
    } else {
        egui::Color32::GRAY
    }
}

fn show_slot_preview_area(ui: &mut egui::Ui, slot: &mut SaveSlotUi, row_rect: egui::Rect) {
    let preview_pos = save_slot_preview_position(ui.ctx(), row_rect);
    egui::Area::new(egui::Id::new(("save-slot-preview", slot.slot)))
        .order(egui::Order::Tooltip)
        .fixed_pos(preview_pos)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                show_slot_preview_contents(ui, slot);
            });
        });
}

fn save_slot_preview_position(ctx: &egui::Context, row_rect: egui::Rect) -> egui::Pos2 {
    const PREVIEW_WIDTH: f32 = 260.0;
    const PREVIEW_HEIGHT: f32 = 270.0;
    const GAP: f32 = 10.0;

    let content_rect = ctx.content_rect();
    let fits_left = row_rect.left() - GAP - PREVIEW_WIDTH >= content_rect.left();
    let fits_right = row_rect.right() + GAP + PREVIEW_WIDTH <= content_rect.right();
    let clamp_x = |x: f32| {
        x.clamp(
            content_rect.left(),
            (content_rect.right() - PREVIEW_WIDTH).max(content_rect.left()),
        )
    };
    let clamp_y = |y: f32| {
        y.clamp(
            content_rect.top(),
            (content_rect.bottom() - PREVIEW_HEIGHT).max(content_rect.top()),
        )
    };

    if fits_left {
        egui::pos2(
            row_rect.left() - GAP - PREVIEW_WIDTH,
            clamp_y(row_rect.top()),
        )
    } else if fits_right {
        egui::pos2(row_rect.right() + GAP, clamp_y(row_rect.top()))
    } else {
        let below = row_rect.bottom() + GAP + PREVIEW_HEIGHT <= content_rect.bottom();
        let y = if below {
            row_rect.bottom() + GAP
        } else {
            row_rect.top() - GAP - PREVIEW_HEIGHT
        };
        egui::pos2(clamp_x(row_rect.left()), clamp_y(y))
    }
}

fn show_slot_preview_contents(ui: &mut egui::Ui, slot: &mut SaveSlotUi) {
    if let Some(preview) = &slot.preview {
        ui.label(format!("Slot {}", slot.slot));
        ui.label(format_full_timestamp(preview.saved_at_unix_secs));

        if let Some(thumbnail_rgb) = preview.thumbnail_rgb.as_deref() {
            if slot.texture.is_none() {
                let image = egui::ColorImage::from_rgb(
                    [
                        preview.thumbnail_width as usize,
                        preview.thumbnail_height as usize,
                    ],
                    thumbnail_rgb,
                );
                slot.texture = Some(ui.ctx().load_texture(
                    format!("save-slot-{}-thumbnail", slot.slot),
                    image,
                    egui::TextureOptions::NEAREST,
                ));
            }

            if let Some(texture) = &slot.texture {
                ui.add(
                    egui::Image::from_texture(texture)
                        .max_width(240.0)
                        .max_height(216.0),
                );
            }
        } else {
            ui.label("No preview available");
        }
    }
}

fn format_short_timestamp(saved_at_unix_secs: u64) -> String {
    let datetime = local_datetime(saved_at_unix_secs);
    format!(
        "{} {} {:02}:{:02} {}",
        month_abbrev(datetime.month()),
        datetime.day(),
        hour12(datetime.hour()),
        datetime.minute(),
        period(datetime.hour())
    )
}

fn format_full_timestamp(saved_at_unix_secs: u64) -> String {
    let datetime = local_datetime(saved_at_unix_secs);
    format!(
        "{} {}, {} {:02}:{:02}:{:02} {}",
        month_abbrev(datetime.month()),
        datetime.day(),
        datetime.year(),
        hour12(datetime.hour()),
        datetime.minute(),
        datetime.second(),
        period(datetime.hour())
    )
}

fn local_datetime(unix_secs: u64) -> OffsetDateTime {
    let unix_secs = i64::try_from(unix_secs).unwrap_or(i64::MAX);
    let datetime =
        OffsetDateTime::from_unix_timestamp(unix_secs).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    datetime.to_offset(offset)
}

fn month_abbrev(month: Month) -> &'static str {
    match month {
        Month::January => "Jan",
        Month::February => "Feb",
        Month::March => "Mar",
        Month::April => "Apr",
        Month::May => "May",
        Month::June => "Jun",
        Month::July => "Jul",
        Month::August => "Aug",
        Month::September => "Sep",
        Month::October => "Oct",
        Month::November => "Nov",
        Month::December => "Dec",
    }
}

fn hour12(hour: u8) -> u8 {
    match hour % 12 {
        0 => 12,
        hour => hour,
    }
}

fn period(hour: u8) -> &'static str {
    if hour < 12 {
        "AM"
    } else {
        "PM"
    }
}

fn upload_screen(texture: &mut glium::texture::texture2d::Texture2d, datavec: &[u8]) {
    let rawimage2d = glium::texture::RawImage2d {
        data: std::borrow::Cow::Borrowed(datavec),
        width: rust_gbe::SCREEN_W as u32,
        height: rust_gbe::SCREEN_H as u32,
        format: glium::texture::ClientFormat::U8U8U8,
    };
    texture.write(
        glium::Rect {
            left: 0,
            bottom: 0,
            width: rust_gbe::SCREEN_W as u32,
            height: rust_gbe::SCREEN_H as u32,
        },
        rawimage2d,
    );
}

/// If `apply` is true, remap the four DMG grayscale shades in `datavec` via `pal` using `scratch`
/// as a reusable buffer; otherwise upload the bytes directly.
fn upload_frame_with_palette(
    texture: &mut glium::texture::texture2d::Texture2d,
    datavec: &[u8],
    apply: bool,
    pal: &DmgPalette,
    scratch: &mut Vec<u8>,
) {
    if apply {
        scratch.clear();
        scratch.extend_from_slice(datavec);
        apply_dmg_palette(scratch, pal);
        upload_screen(texture, scratch);
    } else {
        upload_screen(texture, datavec);
    }
}

fn warn(message: &str) {
    eprintln!("{}", message);
}

fn set_window_size(window: &winit::window::Window, scale: u32) {
    // Add extra height for the menu bar (approximately 30 pixels at 1x scale)
    let menu_bar_height = 30;
    let _ = window.request_inner_size(winit::dpi::LogicalSize::<u32>::from((
        rust_gbe::SCREEN_W as u32 * scale,
        rust_gbe::SCREEN_H as u32 * scale + menu_bar_height,
    )));
}

// Convert 0-100 slider value to linear gain (0.0-1.0) using a perceptual (log-like) curve.
// 50 -> ~0.5 perceived loudness; we map percentage p to gain = (p/100)^(gamma) with gamma ~ 1.5
// This softens high-end changes and gives finer control at low volumes.
fn perceptual_to_linear(v: u8) -> f32 {
    let p = (v as f32) / 100.0;
    if p <= 0.0 {
        0.0
    } else {
        p.powf(1.5)
    } // simple gamma curve
}

// Dynamic mapping using current keybindings
fn dynamic_winit_to_keypad(
    key: winit::keyboard::Key<&str>,
    bindings: &KeyBindings,
) -> Option<rust_gbe::KeypadKey> {
    use winit::keyboard::{Key, NamedKey};
    match key {
        Key::Character(c) => {
            let upc = c.to_uppercase();
            if upc == bindings.a {
                Some(rust_gbe::KeypadKey::A)
            } else if upc == bindings.b {
                Some(rust_gbe::KeypadKey::B)
            } else if upc == bindings.start {
                Some(rust_gbe::KeypadKey::Start)
            } else if upc == bindings.select {
                Some(rust_gbe::KeypadKey::Select)
            } else if upc == bindings.up {
                Some(rust_gbe::KeypadKey::Up)
            } else if upc == bindings.down {
                Some(rust_gbe::KeypadKey::Down)
            } else if upc == bindings.left {
                Some(rust_gbe::KeypadKey::Left)
            } else if upc == bindings.right {
                Some(rust_gbe::KeypadKey::Right)
            } else {
                None
            }
        }
        Key::Named(named) => match named {
            NamedKey::ArrowUp if bindings.up == "ArrowUp" => Some(rust_gbe::KeypadKey::Up),
            NamedKey::ArrowDown if bindings.down == "ArrowDown" => Some(rust_gbe::KeypadKey::Down),
            NamedKey::ArrowLeft if bindings.left == "ArrowLeft" => Some(rust_gbe::KeypadKey::Left),
            NamedKey::ArrowRight if bindings.right == "ArrowRight" => {
                Some(rust_gbe::KeypadKey::Right)
            }
            NamedKey::Space if bindings.select == "Space" => Some(rust_gbe::KeypadKey::Select),
            NamedKey::Enter if bindings.start == "Enter" => Some(rust_gbe::KeypadKey::Start),
            _ => None,
        },
        _ => None,
    }
}

fn key_to_string(key: &winit::keyboard::Key<&str>) -> String {
    use winit::keyboard::{Key, NamedKey};
    match key {
        Key::Character(c) => c.to_uppercase(),
        Key::Named(NamedKey::ArrowUp) => "ArrowUp".into(),
        Key::Named(NamedKey::ArrowDown) => "ArrowDown".into(),
        Key::Named(NamedKey::ArrowLeft) => "ArrowLeft".into(),
        Key::Named(NamedKey::ArrowRight) => "ArrowRight".into(),
        Key::Named(NamedKey::Enter) => "Enter".into(),
        Key::Named(NamedKey::Space) => "Space".into(),
        Key::Named(other) => format!("{other:?}"), // fallback to debug name
        _ => "Unknown".into(),
    }
}

fn matches_capturing(capturing: Option<rust_gbe::KeypadKey>, k: rust_gbe::KeypadKey) -> bool {
    use rust_gbe::KeypadKey::*;
    match (capturing, k) {
        (Some(A), A)
        | (Some(B), B)
        | (Some(Start), Start)
        | (Some(Select), Select)
        | (Some(Up), Up)
        | (Some(Down), Down)
        | (Some(Left), Left)
        | (Some(Right), Right) => true,
        _ => false,
    }
}
