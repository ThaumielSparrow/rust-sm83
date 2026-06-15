use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use cpal::Stream;
use gilrs::{Axis, EventType, Gilrs};
use glium::Surface;
use rust_gbe::device::{read_save_state_preview, SaveStatePreview};
use time::{Month, OffsetDateTime, UtcOffset};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::ModifiersState;
use winit::window::{Fullscreen, WindowId};

use crate::config::GamepadBindings;
use crate::gamepad::{BindTarget, BoundAction, DirectionMux, ResolvedBindings};

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

/// Which tab of the Keybindings window is visible.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BindTab {
    Keyboard,
    Controller,
}

/// Uniform width for binding-value buttons so rows in the keybindings grids line up.
const BIND_BUTTON_WIDTH: f32 = 130.0;

/// Fixed logical width of the keybindings window. Width is not auto-sized:
/// full-width widgets (separators, wrapped labels) track the available width,
/// so measuring it would chase the window size (see the height-only measure
/// in `draw_bind_window`).
const BIND_WINDOW_WIDTH: f32 = 440.0;

/// Read-only reference of fixed keyboard system hotkeys, shown in the
/// keybindings window's Keyboard tab so they're discoverable in-app.
const SYSTEM_HOTKEY_HELP: &[(&str, &str)] = &[
    ("F1–F4", "Save State 1–4"),
    ("F5–F8", "Load State 1–4"),
    ("F9", "FPS overlay"),
    ("F11", "Fullscreen"),
    ("Shift", "Turbo (hold)"),
    ("T", "Turbo (toggle)"),
    ("Backspace", "Rewind (hold)"),
    ("\\", "Rewind (step back)"),
    ("Y", "Linear interpolation"),
    ("P", "Pause"),
    ("M", "Mute"),
    ("Ctrl+R", "Reset"),
    ("Esc", "Exit (double-press)"),
];

/// The keybindings editor lives in its own OS window (with its own GL context
/// and egui instance) so it never has to fit inside the game window at small scales.
struct BindWindow {
    window: winit::window::Window,
    display: glium::Display<glium::glutin::surface::WindowSurface>,
    egui_glium: egui_glium::EguiGlium,
    /// Last height requested to fit content; avoids re-requesting every frame.
    requested_height: Option<f32>,
}

// Unified state machine for ROM selection and emulator run to ensure a single EventLoop
enum RootPhase {
    Selecting {
        rom_path: String,
        browse_requested: bool,
        // Window height (logical px) last requested to fit the egui content,
        // used to avoid re-requesting the same size every frame.
        requested_height: Option<f32>,
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
        gamepad_bindings: GamepadBindings,
        resolved_gamepad: ResolvedBindings,
        gamepad_capturing: Option<BindTarget>,
        direction_mux: DirectionMux,
        bind_tab: BindTab,
        rewinding: bool,
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
    /// None if gamepad support failed to initialize (headless, missing driver).
    gilrs: Option<Gilrs>,
    /// Some while the keybindings window is open.
    bind_window: Option<BindWindow>,
}

impl RootApp {
    pub fn new(scale: u32, pending_rom: Option<PathBuf>) -> Self {
        let default_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_string_lossy().to_string()))
            .unwrap_or_else(|| ".".to_string());
        RootApp {
            window: None,
            display: None,
            egui_glium: None,
            phase: RootPhase::Selecting {
                rom_path: default_dir,
                browse_requested: false,
                requested_height: None,
            },
            scale,
            pending_rom,
            pending_action: None,
            exit_code: EXITCODE_SUCCESS,
            gilrs: match Gilrs::new() {
                Ok(g) => Some(g),
                Err(e) => {
                    eprintln!("gamepad support unavailable: {}", e);
                    None
                }
            },
            bind_window: None,
        }
    }

    /// Opens the keybindings window, or focuses it if already open.
    fn open_bind_window(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(bw) = &self.bind_window {
            bw.window.focus_window();
            return;
        }
        let (window, display) = glium::backend::glutin::SimpleWindowBuilder::new()
            .with_title("Keybindings")
            .with_inner_size(BIND_WINDOW_WIDTH as u32, 320)
            .build(event_loop);
        let egui_glium =
            egui_glium::EguiGlium::new(egui::ViewportId::ROOT, &display, &window, &event_loop);
        window.request_redraw();
        self.bind_window = Some(BindWindow {
            window,
            display,
            egui_glium,
            requested_height: None,
        });
    }

    /// Handles winit events addressed to the keybindings window. Keyboard
    /// capture is processed here (the bind window has focus while rebinding);
    /// Esc cancels an active capture, or closes the window if none is active.
    fn bind_window_event(&mut self, event: WindowEvent) {
        use winit::keyboard::{Key, NamedKey};
        let RootApp { bind_window, phase, gilrs, .. } = self;
        let Some(bw) = bind_window.as_mut() else { return };
        let mut close = false;
        let resp = bw.egui_glium.on_event(&bw.window, &event);
        if resp.repaint {
            bw.window.request_redraw();
        }
        if !resp.consumed {
            match &event {
                WindowEvent::CloseRequested => close = true,
                WindowEvent::Resized(ps) => bw.display.resize((*ps).into()),
                WindowEvent::RedrawRequested => draw_bind_window(bw, phase, gilrs.as_ref()),
                WindowEvent::KeyboardInput { event: keyevent, .. }
                    if keyevent.state == winit::event::ElementState::Pressed =>
                {
                    if let RootPhase::Running { keybindings, capturing, gamepad_capturing, .. } = phase {
                        let logical = keyevent.logical_key.clone();
                        if let Key::Named(NamedKey::Escape) = logical.as_ref() {
                            if gamepad_capturing.is_some() {
                                *gamepad_capturing = None;
                            } else if capturing.is_some() {
                                *capturing = None;
                            } else {
                                close = true;
                            }
                        } else if let Some(kp) = *capturing {
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
                        bw.window.request_redraw();
                    }
                }
                _ => {}
            }
        }
        if close {
            *bind_window = None;
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
        let emu_thread = thread::spawn(move || run_cpu(cpu, frame_sender, recv_events, ui_sender));
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
                gamepad_bindings: cfg.gamepad.clone(),
                resolved_gamepad: crate::gamepad::resolve(&cfg.gamepad),
                gamepad_capturing: None,
                direction_mux: DirectionMux::default(),
                bind_tab: BindTab::Keyboard,
                rewinding: false,
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

    /// Drain all pending gilrs events. Runs every about_to_wait iteration
    /// (the event loop is ControlFlow::Poll). Outside the Running phase events
    /// are discarded so stale presses don't fire when a game starts.
    fn poll_gamepad(&mut self) {
        // Destructure self so phase fields and the other RootApp fields can be
        // borrowed disjointly inside the loop.
        let RootApp { gilrs, phase, window, scale, pending_action, bind_window, .. } = self;
        let Some(gilrs) = gilrs.as_mut() else {
            return;
        };
        let mut transitions: Vec<(rust_gbe::KeypadKey, bool)> = Vec::new();
        let mut any_activity = false;
        while let Some(ev) = gilrs.next_event() {
            let RootPhase::Running {
                sender,
                save_slots,
                latest_frame,
                renderoptions,
                turbo_toggle,
                turbo_held,
                volume,
                pre_mute_volume,
                paused,
                fullscreen,
                fps_overlay,
                rewinding,
                gamepad_bindings,
                resolved_gamepad,
                gamepad_capturing,
                direction_mux,
                ..
            } = &mut *phase
            else {
                continue;
            };
            // (button, pressed) pairs to dispatch; axis events fill `transitions`.
            let mut hotkey: Option<(crate::gamepad::HotkeyAction, bool)> = None;
            transitions.clear();
            match ev.event {
                EventType::ButtonPressed(btn, _) => {
                    any_activity = true;
                    if let Some(target) = gamepad_capturing.take() {
                        // The matching release will dispatch against the new binding; a stray KeyUp / TurboHold(false) is harmless.
                        crate::gamepad::bind(gamepad_bindings, target, btn);
                        *resolved_gamepad = crate::gamepad::resolve(gamepad_bindings);
                        let gb = gamepad_bindings.clone();
                        crate::config::update_config(|c| c.gamepad = gb);
                        continue;
                    }
                    match resolved_gamepad.lookup(btn) {
                        Some(BoundAction::Gb(k)) => direction_mux.set_button(k, true, &mut transitions),
                        Some(BoundAction::Hotkey(a)) => hotkey = Some((a, true)),
                        None => {}
                    }
                }
                EventType::ButtonReleased(btn, _) => {
                    any_activity = true;
                    if gamepad_capturing.is_some() {
                        continue;
                    }
                    match resolved_gamepad.lookup(btn) {
                        Some(BoundAction::Gb(k)) => direction_mux.set_button(k, false, &mut transitions),
                        Some(BoundAction::Hotkey(a)) => hotkey = Some((a, false)),
                        None => {}
                    }
                }
                EventType::AxisChanged(Axis::LeftStickX, v, _) => {
                    direction_mux.set_stick_x(v, &mut transitions);
                    any_activity |= !transitions.is_empty();
                }
                EventType::AxisChanged(Axis::LeftStickY, v, _) => {
                    direction_mux.set_stick_y(v, &mut transitions);
                    any_activity |= !transitions.is_empty();
                }
                _ => {}
            }
            for (k, pressed) in &transitions {
                let _ = sender.send(if *pressed {
                    GBEvent::KeyDown(*k)
                } else {
                    GBEvent::KeyUp(*k)
                });
            }
            if let Some((action, pressed)) = hotkey && let Some(sa) = action.to_system_action(pressed) {
                let outcome = handle_system_action(
                    sa,
                    &mut SysCtx {
                        sender,
                        save_slots,
                        latest_frame,
                        renderoptions,
                        turbo_toggle,
                        turbo_held,
                        volume,
                        pre_mute_volume,
                        paused,
                        fullscreen,
                        fps_overlay,
                        rewinding,
                    },
                );
                if outcome.apply_fullscreen && let Some(win) = window.as_ref() {
                    apply_window_mode(win, *scale, *fullscreen);
                }
                if outcome.request_reset {
                    *pending_action = Some(PendingAction::Reset);
                }
            }
        }
        // Gamepad input arrives outside winit's event stream, so nothing else
        // requests a repaint after it. One redraw per poll that saw input is cheap.
        if any_activity {
            if let Some(w) = window.as_ref() {
                w.request_redraw();
            }
            // Capture completions change binding rows shown in the bind window.
            if let Some(bw) = bind_window.as_ref() {
                bw.window.request_redraw();
            }
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

/// Renders the keybindings UI into its own OS window and resizes the window
/// to fit the active tab's content.
fn draw_bind_window(bw: &mut BindWindow, phase: &mut RootPhase, gilrs: Option<&Gilrs>) {
    use glium::Surface;
    let RootPhase::Running {
        keybindings,
        capturing,
        gamepad_bindings,
        resolved_gamepad,
        gamepad_capturing,
        bind_tab,
        ..
    } = phase
    else {
        return;
    };
    let gilrs_available = gilrs.is_some();
    let pad_name: Option<String> =
        gilrs.and_then(|g| g.gamepads().next().map(|(_, p)| p.name().to_string()));
    let mut content_height: f32 = 0.0;
    bw.egui_glium.run(&bw.window, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            // CentralPanel expands to fill the window; measure a child scope
            // (like the loader screen) so the resize below tracks content.
            let content = ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(bind_tab, BindTab::Keyboard, "Keyboard");
                    ui.selectable_value(bind_tab, BindTab::Controller, "Controller");
                });
                ui.separator();
                let keys = [rust_gbe::KeypadKey::A, rust_gbe::KeypadKey::B, rust_gbe::KeypadKey::Start, rust_gbe::KeypadKey::Select,
                    rust_gbe::KeypadKey::Up, rust_gbe::KeypadKey::Down, rust_gbe::KeypadKey::Left, rust_gbe::KeypadKey::Right];
                let key_label = |k: rust_gbe::KeypadKey| match k { rust_gbe::KeypadKey::A=>"A", rust_gbe::KeypadKey::B=>"B", rust_gbe::KeypadKey::Start=>"Start", rust_gbe::KeypadKey::Select=>"Select", rust_gbe::KeypadKey::Up=>"Up", rust_gbe::KeypadKey::Down=>"Down", rust_gbe::KeypadKey::Left=>"Left", rust_gbe::KeypadKey::Right=>"Right" };
                match *bind_tab {
                    BindTab::Keyboard => {
                        ui.label("Click a binding, then press a key (Esc to cancel capture). Reserved keys can't be used.");
                        ui.add_space(4.0);
                        egui::Grid::new("kb_bind_grid").num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
                            for k in keys {
                                ui.label(key_label(k));
                                let active = matches_capturing(*capturing, k);
                                let val = binding_value(keybindings, k);
                                let conflict = is_reserved_key_name(&val);
                                let label = if active { "(press key)".to_string() } else { val.clone() };
                                let mut button = egui::Button::new(label).min_size(egui::vec2(BIND_BUTTON_WIDTH, 0.0));
                                if conflict {
                                    button = button.fill(egui::Color32::from_rgb(100,0,0));
                                }
                                if ui.add(button).clicked() {
                                    *capturing = Some(k);
                                    *gamepad_capturing = None; // keyboard capture replaces any gamepad capture
                                }
                                if conflict {
                                    ui.colored_label(egui::Color32::RED, "Conflicts with system keybind");
                                }
                                ui.end_row();
                            }
                        });
                        if capturing.is_some() && ui.button("Cancel Capture").clicked() { *capturing=None; }
                        ui.add_space(4.0);
                        egui::CollapsingHeader::new("System hotkeys (fixed)")
                            .default_open(false)
                            .show(ui, |ui| {
                                egui::Grid::new("kb_hotkey_grid")
                                    .num_columns(2)
                                    .spacing([12.0, 4.0])
                                    .show(ui, |ui| {
                                        for (keys, action) in SYSTEM_HOTKEY_HELP {
                                            ui.label(*keys);
                                            ui.label(*action);
                                            ui.end_row();
                                        }
                                    });
                            });
                    }
                    BindTab::Controller => {
                        match (&pad_name, gilrs_available) {
                            (Some(n), _) => { ui.label(format!("Connected: {}", n)); }
                            (None, true) => { ui.colored_label(egui::Color32::GRAY, "No controller detected"); }
                            (None, false) => { ui.colored_label(egui::Color32::GRAY, "Controller unavailable"); }
                        }
                        ui.label("Click a binding, then press a controller button (Esc to cancel capture).");
                        ui.add_space(4.0);
                        egui::Grid::new("pad_bind_grid").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                            for k in keys {
                                ui.label(key_label(k));
                                let name = crate::gamepad::keypad_key_name(k);
                                let active = matches!(gamepad_capturing, Some(BindTarget::Gb(g)) if *g == k);
                                let bound = gamepad_bindings.buttons.get(name).cloned().unwrap_or_else(|| "Unbound".to_string());
                                let label = if active { "(press button)".to_string() } else { bound };
                                if ui.add(egui::Button::new(label).min_size(egui::vec2(BIND_BUTTON_WIDTH, 0.0))).clicked() {
                                    *gamepad_capturing = Some(BindTarget::Gb(k));
                                    *capturing = None; // gamepad capture replaces any keyboard capture
                                }
                                ui.end_row();
                            }
                        });
                        ui.add_space(4.0);
                        egui::CollapsingHeader::new("System hotkeys").default_open(false).show(ui, |ui| {
                            egui::Grid::new("pad_hotkey_grid").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                                for action in crate::gamepad::HotkeyAction::all() {
                                    ui.label(action.label());
                                    let active = matches!(gamepad_capturing, Some(BindTarget::Hotkey(a)) if *a == action);
                                    let bound = gamepad_bindings.hotkeys.get(action.name()).cloned().unwrap_or_else(|| "Unbound".to_string());
                                    let label = if active { "(press button)".to_string() } else { bound };
                                    if ui.add(egui::Button::new(label).min_size(egui::vec2(BIND_BUTTON_WIDTH, 0.0))).clicked() {
                                        *gamepad_capturing = Some(BindTarget::Hotkey(action));
                                        *capturing = None;
                                    }
                                    ui.end_row();
                                }
                            });
                        });
                        ui.horizontal(|ui| {
                            if gamepad_capturing.is_some() && ui.button("Cancel Controller Capture").clicked() {
                                *gamepad_capturing = None;
                            }
                            if ui.button("Reset Controller Defaults").clicked() {
                                *gamepad_bindings = GamepadBindings::default();
                                *resolved_gamepad = crate::gamepad::resolve(gamepad_bindings);
                                *gamepad_capturing = None;
                                let gb = gamepad_bindings.clone();
                                crate::config::update_config(|c| c.gamepad = gb);
                            }
                        });
                    }
                }
            });
            content_height = content.response.rect.height();
        });
    });
    let mut target = bw.display.draw();
    target.clear_color(0.1, 0.1, 0.1, 1.0);
    bw.egui_glium.paint(&bw.display, &mut target);
    let _ = target.finish();
    // Nothing else drives redraws of this window, so honor egui's repaint
    // requests (collapsing-header animation, etc.) ourselves.
    if bw.egui_glium.egui_ctx().has_requested_repaint() {
        bw.window.request_redraw();
    }
    // Fit the window height to the active tab's content (tab switches and the
    // hotkeys expander change it); width stays fixed (see BIND_WINDOW_WIDTH).
    // Only re-request when the target changes so a denied/clamped request
    // can't loop.
    let desired = (content_height + 24.0).clamp(120.0, 800.0);
    if bw
        .requested_height
        .is_none_or(|h| (h - desired).abs() > 2.0)
    {
        bw.requested_height = Some(desired);
        let _ = bw
            .window
            .request_inner_size(winit::dpi::LogicalSize::new(BIND_WINDOW_WIDTH, desired));
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

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Belt-and-suspenders: runs once when the event loop is destroyed,
        // catching any exit path that calls event_loop.exit() without first
        // going through CloseRequested. Idempotent — a no-op if already stopped.
        self.stop_emulator();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        use winit::event::ElementState::{Pressed, Released};
        use winit::keyboard::{Key, NamedKey};

        // The keybindings window has its own egui instance and event handling.
        if self.bind_window.as_ref().is_some_and(|b| b.window.id() == window_id) {
            self.bind_window_event(event);
            return;
        }

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
                // Join the emulator thread so its flush_to_disk / battery-RAM
                // save runs before we tear down the event loop. Without this,
                // the most common quit path (OS close button) races process
                // exit and can drop the latest SRAM / auto-save.
                self.stop_emulator();
                event_loop.exit();
            }
            (
                RootPhase::Selecting {
                    rom_path,
                    browse_requested,
                    requested_height,
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
                // Measured height of the egui content this frame (logical px),
                // used to grow/shrink the window to fit the recent-ROMs list.
                let mut content_height: f32 = 0.0;
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
                            let last = ui.label("Tip: drag a .gb/.gbc file onto this window to load it.");
                            // CentralPanel expands its ui to fill the window, so
                            // ui.min_rect() would measure the window itself and
                            // feed back into the resize below (endless growth).
                            // Measure top-of-content to bottom-of-last-widget.
                            content_height = last.rect.bottom() - ui.max_rect().top();
                        });
                    });
                    // Paint after UI
                    let mut target = display.draw();
                    target.clear_color(0.1, 0.1, 0.1, 1.0);
                    egui_glium.paint(display, &mut target);
                    let _ = target.finish();

                    // Resize the window to fit the content (the recent-ROMs list
                    // grows it past the initial 220px). Only request when the
                    // target changes so a denied/clamped request can't loop.
                    let desired = (content_height + 24.0).clamp(220.0, 700.0);
                    if requested_height.is_none_or(|h| (h - desired).abs() > 2.0) {
                        *requested_height = Some(desired);
                        let _ = window.request_inner_size(winit::dpi::LogicalSize::new(
                            600.0, desired,
                        ));
                    }
                }
                if let Some(p) = launch_path {
                    self.start_game_from_path(p);
                }
                if quit_requested {
                    self.exit_code = EXITCODE_SUCCESS;
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
                    last_escape,
                    turbo_toggle,
                    turbo_held,
                    volume,
                    modifiers,
                    paused,
                    pre_mute_volume,
                    fullscreen,
                    fps_overlay,
                    rewinding,
                    gamepad_capturing,
                    ..
                },
                WindowEvent::KeyboardInput {
                    event: keyevent, ..
                },
            ) => {
                let state = keyevent.state;
                let logical = keyevent.logical_key.clone();
                if gamepad_capturing.is_some() && let (winit::event::ElementState::Pressed, Key::Named(NamedKey::Escape)) = (state, logical.as_ref()) {
                    *gamepad_capturing = None;
                    return;
                }
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
                if let Some(action) =
                    crate::input::system_action_for(&logical.as_ref(), state, *modifiers)
                {
                    let outcome = handle_system_action(
                        action,
                        &mut SysCtx {
                            sender,
                            save_slots,
                            latest_frame,
                            renderoptions,
                            turbo_toggle,
                            turbo_held,
                            volume,
                            pre_mute_volume,
                            paused,
                            fullscreen,
                            fps_overlay,
                            rewinding,
                        },
                    );
                    // Snapshot values we'll need outside the phase borrow.
                    let fs = *fullscreen;
                    let sc = self.scale;
                    if outcome.apply_fullscreen && let Some(win) = &self.window {
                        apply_window_mode(win, sc, fs);
                    }
                    if outcome.request_reset {
                        self.pending_action = Some(PendingAction::Reset);
                    }
                    return;
                }
                match (state, logical.as_ref()) {
                    // Escape: require double-press to exit. (The keybindings
                    // window is a separate OS window with its own Esc handling.)
                    (Pressed, Key::Named(NamedKey::Escape)) => {
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
                    turbo_toggle,
                    turbo_setting,
                    volume,
                    paused,
                    fullscreen,
                    fps_overlay,
                    rewinding,
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
                let mut open_keybindings = false;
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
                                    if ui.button("Keybindings...").clicked() { open_keybindings = true; }
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
                        if *rewinding {
                            egui::Area::new(egui::Id::new("rewind_overlay"))
                                .anchor(
                                    egui::Align2::CENTER_TOP,
                                    egui::vec2(0.0, menu_bar_height + 4.0),
                                )
                                .show(ctx, |ui| {
                                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                                        ui.colored_label(
                                            egui::Color32::from_rgb(255, 80, 80),
                                            "⏪ REWIND",
                                        );
                                    });
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
                if open_keybindings {
                    self.open_bind_window(event_loop);
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
        self.poll_gamepad();
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
        // Gamepad-triggered Reset is queued as a pending action; window_event's
        // drain won't run until the next OS event, so drain here too.
        self.drain_pending_action();
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

/// Side effects of a system action that can't run while `self.phase` is
/// mutably borrowed; the caller applies them after the borrow ends.
#[derive(Default)]
struct SysOutcome {
    request_reset: bool,
    apply_fullscreen: bool,
}

/// Borrowed `Running`-phase fields needed to execute a `SystemAction`.
/// Shared by the keyboard path (window_event) and the gamepad path
/// (poll_gamepad) so dispatch logic exists exactly once.
struct SysCtx<'a> {
    sender: &'a mpsc::Sender<GBEvent>,
    save_slots: &'a mut SaveSlotCache,
    latest_frame: &'a Option<Arc<Vec<u8>>>,
    renderoptions: &'a mut RenderOptions,
    turbo_toggle: &'a mut bool,
    turbo_held: &'a mut bool,
    volume: &'a mut u8,
    pre_mute_volume: &'a mut Option<u8>,
    paused: &'a mut bool,
    fullscreen: &'a mut bool,
    fps_overlay: &'a mut bool,
    rewinding: &'a mut bool,
}

fn handle_system_action(action: crate::input::SystemAction, ctx: &mut SysCtx) -> SysOutcome {
    use crate::input::SystemAction;
    let mut outcome = SysOutcome::default();
    match action {
        SystemAction::SaveState(s) => {
            request_save_state(ctx.sender, ctx.save_slots, s, ctx.latest_frame);
        }
        SystemAction::LoadState(s) => {
            let _ = ctx.sender.send(GBEvent::LoadState(s));
        }
        SystemAction::TurboHold(press) => {
            if press {
                if !*ctx.turbo_toggle && !*ctx.turbo_held {
                    let _ = ctx.sender.send(GBEvent::SpeedUp);
                }
                *ctx.turbo_held = true;
            } else {
                *ctx.turbo_held = false;
                if !*ctx.turbo_toggle {
                    let _ = ctx.sender.send(GBEvent::SpeedDown);
                }
            }
        }
        SystemAction::TurboToggle => {
            *ctx.turbo_toggle = !*ctx.turbo_toggle;
            if *ctx.turbo_toggle {
                if !*ctx.turbo_held {
                    let _ = ctx.sender.send(GBEvent::SpeedUp);
                }
            } else if !*ctx.turbo_held {
                let _ = ctx.sender.send(GBEvent::SpeedDown);
            }
        }
        SystemAction::ToggleInterpolation => {
            ctx.renderoptions.linear_interpolation = !ctx.renderoptions.linear_interpolation;
        }
        SystemAction::TogglePause => {
            *ctx.paused = !*ctx.paused;
            let _ = ctx.sender.send(GBEvent::SetPaused(*ctx.paused));
        }
        SystemAction::Reset => {
            outcome.request_reset = true;
        }
        SystemAction::ToggleFullscreen => {
            *ctx.fullscreen = !*ctx.fullscreen;
            let fs = *ctx.fullscreen;
            crate::config::update_config(|c| c.fullscreen = fs);
            outcome.apply_fullscreen = true;
        }
        SystemAction::ToggleMute => {
            if let Some(restored) = ctx.pre_mute_volume.take() {
                *ctx.volume = restored;
            } else {
                *ctx.pre_mute_volume = Some(*ctx.volume);
                *ctx.volume = 0;
            }
            let _ = ctx.sender.send(GBEvent::UpdateVolume(perceptual_to_linear(*ctx.volume)));
            let v = *ctx.volume;
            crate::config::update_config(|c| c.volume = v);
        }
        SystemAction::ToggleFpsOverlay => {
            *ctx.fps_overlay = !*ctx.fps_overlay;
            let on = *ctx.fps_overlay;
            crate::config::update_config(|c| c.fps_overlay = on);
        }
        SystemAction::RewindHold(press) => {
            *ctx.rewinding = press;
            let _ = ctx.sender.send(GBEvent::SetRewinding(press));
        }
        SystemAction::RewindStep => {
            let _ = ctx.sender.send(GBEvent::RewindStep);
        }
    }
    outcome
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
    // Single lookup path: render the key to its string form and compare against
    // each binding. This lets a user bind A to "ArrowUp" or Up to "X" without
    // duplicating per-NamedKey checks.
    let key_str = key_to_string(&key);
    let key_upper = key_str.to_uppercase();
    if key_upper == bindings.a.to_uppercase() { return Some(rust_gbe::KeypadKey::A); }
    if key_upper == bindings.b.to_uppercase() { return Some(rust_gbe::KeypadKey::B); }
    if key_upper == bindings.start.to_uppercase() { return Some(rust_gbe::KeypadKey::Start); }
    if key_upper == bindings.select.to_uppercase() { return Some(rust_gbe::KeypadKey::Select); }
    if key_upper == bindings.up.to_uppercase() { return Some(rust_gbe::KeypadKey::Up); }
    if key_upper == bindings.down.to_uppercase() { return Some(rust_gbe::KeypadKey::Down); }
    if key_upper == bindings.left.to_uppercase() { return Some(rust_gbe::KeypadKey::Left); }
    if key_upper == bindings.right.to_uppercase() { return Some(rust_gbe::KeypadKey::Right); }
    None
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
