use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::Stream;
use gilrs::{Axis, EventType, Gilrs};
use glium::Surface;
use rust_gbe::device::{read_save_state_preview, SaveStatePreview};
use time::{Month, OffsetDateTime, UtcOffset};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Fullscreen, WindowId};

use crate::config::GamepadBindings;
use crate::gamepad::{BindTarget, BoundAction, DirectionMux, ResolvedBindings};

use crate::audio::init_audio;
use crate::config::{binding_value, config_path, Config, DmgPalettePreset, KeyBindings, TurboSetting};
use crate::emulator::{construct_cpu_auto, run_cpu, GBEvent, GuiEvent};
use crate::input::is_reserved_key_name;
use crate::palette::{apply_dmg_palette, palette_for_preset, DmgPalette};
use crate::toast::{ToastKind, ToastQueue};

mod actions;
mod aux_windows;
mod display;
mod states;

use actions::*;
use aux_windows::*;
use display::*;
use states::*;

pub const EXITCODE_SUCCESS: i32 = 0;
pub const EXITCODE_CPULOADFAILS: i32 = 2;

// Unified state machine for ROM selection and emulator run to ensure a single EventLoop
enum RootPhase {
    Selecting {
        rom_path: String,
        browse_requested: bool,
        // Window height (logical px) last requested to fit the egui content,
        // used to avoid re-requesting the same size every frame.
        requested_height: Option<f32>,
        /// Why the last ROM failed to load, shown under the path field.
        load_error: Option<String>,
    },
    Running(Box<Running>),
}

/// Everything the GUI holds for a running game.
struct Running {
    texture: glium::texture::texture2d::Texture2d,
    sender: mpsc::Sender<GBEvent>,
    receiver: Receiver<Arc<Vec<u8>>>,
    ui_receiver: Receiver<GuiEvent>,
    save_slots: SaveSlotCache,
    latest_frame: Option<Arc<Vec<u8>>>,
    renderoptions: RenderOptions,
    screen: ScreenRenderer,
    cgb_color_correction: bool,
    running: bool,
    keybindings: KeyBindings,
    capturing: Option<rust_gbe::KeypadKey>,
    _audio: Option<Stream>,
    /// Queue/health view of `_audio`; None when running on the null player.
    audio_clock: Option<crate::audio::AudioClock>,
    /// When to next try (re)opening an audio device, if the current one
    /// failed or none was available.
    audio_retry_at: Option<Instant>,
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
    toasts: ToastQueue,
    integer_scaling: bool,
    auto_resume: bool,
    pause_on_focus_loss: bool,
    /// True while paused because the window lost focus (resume on regain).
    focus_paused: bool,
    cheats: Vec<crate::cheats::Cheat>,
    cheat_key: String,
    cheat_entry: String,
    cheat_entry_label: String,
    cheat_error: Option<&'static str>,
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
    bind_window: Option<AuxWindow>,
    /// Some while the cheats window is open.
    cheat_window: Option<AuxWindow>,
    /// In-memory copy of `Config::recent_roms` so the menus don't re-read
    /// config.json every frame.
    recent_roms: Vec<String>,
    /// Main window position, saved to config on exit.
    window_pos: Option<(i32, i32)>,
    /// Lets the emulator thread wake the (otherwise sleeping) event loop.
    proxy: EventLoopProxy<UserEvent>,
    /// Rumble cart motor change from the emulator, applied in about_to_wait.
    pending_rumble: Option<bool>,
    /// Playing force-feedback effect while a rumble cart's motor is on.
    rumble_effect: Option<gilrs::ff::Effect>,
}

/// How often to retry opening an audio device while there is none.
const AUDIO_RETRY: Duration = Duration::from_secs(2);

/// Custom event-loop events.
#[derive(Debug)]
pub enum UserEvent {
    /// The emulator thread sent a frame or a `GuiEvent`, or exited.
    /// `about_to_wait` does the actual draining.
    Wake,
}

impl RootApp {
    pub fn new(scale: u32, pending_rom: Option<PathBuf>, proxy: EventLoopProxy<UserEvent>) -> Self {
        let cfg = Config::load(&config_path());
        // Start the picker in the folder of the most recent ROM.
        let default_dir = cfg
            .recent_roms
            .first()
            .and_then(|p| std::path::Path::new(p).parent())
            .map(|p| p.to_string_lossy().to_string())
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|p| p.to_string_lossy().to_string()))
            })
            .unwrap_or_else(|| ".".to_string());
        RootApp {
            window: None,
            display: None,
            egui_glium: None,
            phase: RootPhase::Selecting {
                rom_path: default_dir,
                browse_requested: false,
                requested_height: None,
                load_error: crate::config::take_load_warning(),
            },
            scale,
            pending_rom,
            pending_action: None,
            exit_code: EXITCODE_SUCCESS,
            gilrs: match Gilrs::new() {
                Ok(g) => Some(g),
                Err(e) => {
                    crate::log::warn(format!("gamepad support unavailable: {}", e));
                    None
                }
            },
            bind_window: None,
            cheat_window: None,
            recent_roms: cfg.recent_roms,
            proxy,
            pending_rumble: None,
            rumble_effect: None,
            window_pos: cfg.window_pos,
        }
    }

    /// Opens the keybindings window, or focuses it if already open.
    fn open_bind_window(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(bw) = &self.bind_window {
            bw.window.focus_window();
            return;
        }
        self.bind_window = Some(AuxWindow::open(event_loop, "Keybindings", BIND_WINDOW_WIDTH, 320));
    }

    /// Opens the cheats window, or focuses it if already open.
    fn open_cheat_window(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(cw) = &self.cheat_window {
            cw.window.focus_window();
            return;
        }
        self.cheat_window = Some(AuxWindow::open(event_loop, "Cheats", CHEAT_WINDOW_WIDTH, 240));
    }

    /// Winit events addressed to the cheats window. Esc closes it.
    fn cheat_window_event(&mut self, event: WindowEvent) {
        use winit::keyboard::{Key, NamedKey};
        let RootApp { cheat_window, phase, .. } = self;
        let Some(cw) = cheat_window.as_mut() else { return };
        let mut close = false;
        let resp = cw.egui_glium.on_event(&cw.window, &event);
        if resp.repaint {
            cw.window.request_redraw();
        }
        if !resp.consumed {
            match &event {
                WindowEvent::CloseRequested => close = true,
                WindowEvent::Resized(ps) => cw.display.resize((*ps).into()),
                WindowEvent::RedrawRequested => draw_cheat_window(cw, phase),
                WindowEvent::KeyboardInput { event: keyevent, .. }
                    if keyevent.state == winit::event::ElementState::Pressed =>
                {
                    if let Key::Named(NamedKey::Escape) = keyevent.logical_key.as_ref() {
                        close = true;
                    }
                }
                _ => {}
            }
        }
        if close {
            *cheat_window = None;
        }
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
                    if let RootPhase::Running(running) = phase {
                        let Running { keybindings, capturing, gamepad_capturing, .. } = &mut **running;
                        let logical = keyevent.logical_key.clone();
                        if let Key::Named(NamedKey::Escape) = logical.as_ref() {
                            if gamepad_capturing.is_some() {
                                *gamepad_capturing = None;
                            } else if capturing.is_some() {
                                *capturing = None;
                            } else {
                                close = true;
                            }
                        } else if let Some(kp) = *capturing
                            && !keyevent.repeat
                            && keybindings.assign(kp, key_to_string(&logical.as_ref()))
                        {
                            // A refused (reserved) key leaves capture active.
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
                    RootPhase::Running(running) => Some(running.rom_path.clone()),
                    _ => None,
                };
                if let Some(p) = path {
                    // Reset must boot fresh, not re-resume the just-flushed state.
                    self.start_game_from_path(p, false);
                }
            }
            Some(PendingAction::LoadRom(p)) => {
                self.start_game_from_path(p, true);
            }
            None => {}
        }
    }

    /// `resume`: attempt to continue from the auto-save written at last
    /// shutdown. Reset passes false — it must not re-load the state that
    /// `stop_emulator` just flushed.
    ///
    /// If a game is already running it keeps running until the new ROM has
    /// loaded, so a bad or missing file only produces an error toast.
    fn start_game_from_path(&mut self, rom_path: PathBuf, resume: bool) {
        // Relaunching the running ROM (reset, or picking it from recents):
        // stop first so the outgoing emulator flushes battery RAM and the
        // auto-save before the new device reads them.
        let same_rom =
            matches!(&self.phase, RootPhase::Running(running) if running.rom_path == rom_path);
        if same_rom {
            self.stop_emulator();
        }
        // Always run in (CGB-capable) mode; attempt CGB first, fallback to classic if needed.
        let filename = rom_path.to_string_lossy().to_string();
        let (mut cpu, load_warning) = match construct_cpu_auto(&filename) {
            Ok(pair) => pair,
            Err(msg) => {
                self.report_load_failure(&rom_path, msg);
                return;
            }
        };
        self.stop_emulator();
        let cfg = Config::load(&config_path());
        let mut toasts = ToastQueue::default();
        if let Some(w) = load_warning {
            toasts.push(w, ToastKind::Info);
        }
        if resume && cfg.auto_resume && cpu.has_auto_save() {
            match cpu.load_auto_save() {
                Ok(()) => toasts.push("Resumed last session", ToastKind::Success),
                Err(_) => toasts.push(
                    "Couldn't resume last session — starting fresh",
                    ToastKind::Info,
                ),
            }
        }
        let is_color = cpu.is_cgb_mode();
        let cheat_key = crate::cheats::cheat_key(&cpu.romname(), &rom_path);
        let cheats = cfg.cheats.get(&cheat_key).cloned().unwrap_or_default();
        // Enable audio by default; if no device is available, fall back to a
        // discarding player so the APU is still emulated (games poll NR52 etc.).
        let mut audio_stream = None;
        let mut audio_clock = None;
        if let Some((player, s, clock)) = init_audio() {
            cpu.enable_audio(player, true);
            audio_stream = Some(s);
            audio_clock = Some(clock);
        } else {
            warn("Audio disabled: no output device available");
            cpu.enable_audio(crate::audio::null_player(), true);
        }
        let _ = cpu.romname();
        let save_slots = SaveSlotCache::from_paths(
            (1..=8)
                .map(|slot| (slot, cpu.save_state_slot_path(slot)))
                .collect(),
        );
        let (sender, recv_events) = mpsc::channel();
        let (frame_sender, frame_receiver) = mpsc::sync_channel(1);
        let (ui_sender, ui_receiver) = mpsc::channel();
        let proxy = self.proxy.clone();
        let wake = Box::new(move || {
            let _ = proxy.send_event(UserEvent::Wake);
        });
        let thread_clock = audio_clock.clone();
        let emu_thread = thread::spawn(move || {
            run_cpu(cpu, frame_sender, recv_events, ui_sender, wake, thread_clock)
        });
        let audio_retry_at = audio_clock.is_none().then(|| Instant::now() + AUDIO_RETRY);
        if let Some(display) = &self.display {
            let texture = glium::texture::texture2d::Texture2d::empty_with_format(
                display,
                glium::texture::UncompressedFloatFormat::U8U8U8,
                glium::texture::MipmapsOption::NoMipmap,
                rust_gbe::SCREEN_W as u32,
                rust_gbe::SCREEN_H as u32,
            )
            .unwrap();
            let initial_scale = cfg.scale;
            self.scale = initial_scale;

            // Push to recent ROMs.
            crate::config::update_config(|c| c.push_recent(&rom_path));
            crate::config::push_recent(&mut self.recent_roms, &rom_path);

            self.phase = RootPhase::Running(Box::new(Running {
                texture,
                sender,
                receiver: frame_receiver,
                ui_receiver,
                save_slots,
                latest_frame: None,
                renderoptions: RenderOptions {
                    filter: cfg.screen_filter,
                    lcd_grid: cfg.lcd_grid,
                },
                screen: ScreenRenderer::new(display),
                cgb_color_correction: cfg.cgb_color_correction,
                running: true,
                keybindings: cfg.keybindings.clone(),
                capturing: None,
                _audio: audio_stream,
                audio_clock,
                audio_retry_at,
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
                toasts,
                integer_scaling: cfg.integer_scaling,
                auto_resume: cfg.auto_resume,
                pause_on_focus_loss: cfg.pause_on_focus_loss,
                focus_paused: false,
                cheats,
                cheat_key,
                cheat_entry: String::new(),
                cheat_entry_label: String::new(),
                cheat_error: None,
            }));
            if let RootPhase::Running(running) = &self.phase {
                let Running { sender, cheats, .. } = & **running;
                let _ = sender.send(GBEvent::UpdateTurbo(cfg.turbo));
                let _ = sender.send(GBEvent::SetColorCorrection(cfg.cgb_color_correction));
                let _ = sender.send(GBEvent::UpdateVolume(perceptual_to_linear(cfg.volume)));
                let _ = sender.send(GBEvent::SetCheats(crate::cheats::enabled_pokes(cheats)));
            }
            // Now that we've transitioned to Running, resize/configure window.
            if let Some(win) = &self.window {
                apply_window_mode(win, self.scale, cfg.fullscreen);
            }
        } else {
            self.exit_code = EXITCODE_CPULOADFAILS;
        }
    }

    /// Mirror a rumble cartridge's motor on every force-feedback gamepad.
    fn set_rumble(&mut self, on: bool) {
        // Dropping the effect stops it.
        self.rumble_effect = None;
        let Some(gilrs) = self.gilrs.as_mut() else {
            return;
        };
        if !on {
            return;
        }
        let pads: Vec<_> = gilrs
            .gamepads()
            .filter(|(_, g)| g.is_ff_supported())
            .map(|(id, _)| id)
            .collect();
        if pads.is_empty() {
            return;
        }
        let effect = gilrs::ff::EffectBuilder::new()
            .add_effect(gilrs::ff::BaseEffect {
                kind: gilrs::ff::BaseEffectType::Strong { magnitude: 40_000 },
                ..Default::default()
            })
            .gamepads(&pads)
            .finish(gilrs);
        match effect {
            Ok(e) if e.play().is_ok() => self.rumble_effect = Some(e),
            Ok(_) => {}
            Err(e) => crate::log::warn(format!("rumble unavailable: {e}")),
        }
    }

    /// Recover audio when the output device goes away (e.g. headphones
    /// unplugged) or appears after starting without one: open the current
    /// default device and hand the emulator a new player. APU state is kept.
    fn maintain_audio(&mut self) {
        let RootPhase::Running(running) = &mut self.phase else {
            return;
        };
        let Running { sender, _audio, audio_clock, audio_retry_at, toasts, .. } = &mut **running;
        if audio_clock.as_ref().is_some_and(|c| c.failed()) {
            *_audio = None;
            *audio_clock = None;
            let _ = sender.send(GBEvent::SetAudio(crate::audio::null_player(), None));
            *audio_retry_at = Some(Instant::now());
        }
        let Some(at) = *audio_retry_at else {
            return;
        };
        if Instant::now() < at {
            return;
        }
        match init_audio() {
            Some((player, stream, clock)) => {
                let _ = sender.send(GBEvent::SetAudio(player, Some(clock.clone())));
                *_audio = Some(stream);
                *audio_clock = Some(clock);
                *audio_retry_at = None;
                toasts.push("Audio device connected", ToastKind::Info);
            }
            None => *audio_retry_at = Some(Instant::now() + AUDIO_RETRY),
        }
    }

    /// Surface a ROM that failed to load. A running game keeps running and
    /// gets an error toast; otherwise fall back to the picker with the error.
    fn report_load_failure(&mut self, rom_path: &std::path::Path, msg: &str) {
        let name = rom_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| rom_path.display().to_string());
        let text = format!("Couldn't load {name}: {msg}");
        if let RootPhase::Running(running) = &mut self.phase
            && running.emu_thread.is_some()
        {
            running.toasts.push(text, ToastKind::Error);
            return;
        }
        self.phase = RootPhase::Selecting {
            rom_path: rom_path.to_string_lossy().into_owned(),
            browse_requested: false,
            requested_height: None,
            load_error: Some(text),
        };
        if let Some(win) = &self.window {
            win.set_fullscreen(None);
            let _ = win.request_inner_size(winit::dpi::LogicalSize::new(600.0, 220.0));
        }
    }

    /// Send Shutdown to the emulator worker and join its thread. Leaves `self.phase`
    /// in `Running` (with `emu_thread = None`) so the caller can immediately transition
    /// to a new phase (Selecting or another Running via `start_game_from_path`).
    pub fn stop_emulator(&mut self) {
        self.rumble_effect = None;
        let handle = match &mut self.phase {
            RootPhase::Running(running) => {
                let _ = running.sender.send(GBEvent::Shutdown);
                running.emu_thread.take()
            }
            _ => None,
        };
        if let Some(h) = handle {
            let _ = h.join();
        }
    }

    /// Drain all pending gilrs events. Runs every about_to_wait iteration
    /// (on a short timer while a pad is connected). Outside the Running phase events
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
            let RootPhase::Running(running) = &mut *phase
            else {
                continue;
            };
            let Running { sender,
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
                toasts,
                .. } = &mut **running;
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
                // An unplugged pad never sends its releases.
                EventType::Disconnected => {
                    release_held_inputs(sender, *turbo_toggle, turbo_held, rewinding);
                    *direction_mux = DirectionMux::default();
                    any_activity = true;
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
                        toasts,
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

fn rom_path_is_supported(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|s| s.to_str()).map(|s| s.to_ascii_lowercase()),
        Some(ref ext) if ext == "gb" || ext == "gbc" || ext == "zip"
    )
}

impl ApplicationHandler<UserEvent> for RootApp {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: UserEvent) {
        // Nothing to do here: about_to_wait runs next and drains the channels.
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let (window, display) = glium::backend::glutin::SimpleWindowBuilder::new()
                .with_title("Game Boy Emulator")
                .with_inner_size(600, 220)
                .build(event_loop);
            // Restore the last position, unless that spot is no longer on
            // any monitor (e.g. an unplugged second screen).
            if let Some((x, y)) = self.window_pos
                && position_on_a_monitor(&window, x, y)
            {
                window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
            }
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
                self.start_game_from_path(p, true);
            }
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Belt-and-suspenders: runs once when the event loop is destroyed,
        // catching any exit path that calls event_loop.exit() without first
        // going through CloseRequested. Idempotent — a no-op if already stopped.
        self.stop_emulator();
        if let Some(pos) = self.window_pos {
            crate::config::update_config(|c| c.window_pos = Some(pos));
        }
        crate::config::flush_pending_config(true);
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
        // Likewise the cheats window.
        if self.cheat_window.as_ref().is_some_and(|c| c.window.id() == window_id) {
            self.cheat_window_event(event);
            return;
        }

        if let WindowEvent::Moved(p) = &event
            && self.window.as_ref().is_some_and(|w| w.fullscreen().is_none())
        {
            self.window_pos = Some((p.x, p.y));
        }

        // Pass events to egui in all phases (menus in Running phase)
        if let Some(egui_glium) = &mut self.egui_glium {
            let resp = egui_glium.on_event(self.window.as_ref().unwrap(), &event);
            // egui flags every RedrawRequested as needing a repaint; acting on
            // that would redraw forever. Its real repaint requests are honored
            // after painting (has_requested_repaint).
            if resp.repaint && !matches!(event, WindowEvent::RedrawRequested) {
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
                    load_error,
                },
                WindowEvent::RedrawRequested,
            ) => {
                if *browse_requested {
                    *browse_requested = false;
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("Game Boy ROMs", &["gb", "gbc", "zip"])
                        .add_filter("All files", &["*"])
                        .set_directory(&rom_path)
                        .pick_file()
                    {
                        *rom_path = p.to_string_lossy().to_string();
                    }
                }
                let mut launch_path: Option<PathBuf> = None;
                let mut quit_requested = false;
                let recent_roms = self.recent_roms.clone();
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
                            if let Some(err) = load_error.as_deref() {
                                ui.colored_label(egui::Color32::RED, err);
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
                    if egui_glium.egui_ctx().has_requested_repaint() {
                        window.request_redraw();
                    }

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
                    self.start_game_from_path(p, true);
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
                    self.start_game_from_path(path, true);
                }
            }
            // Key-up events are lost while unfocused, so release everything
            // held (buttons, momentary turbo, rewind) instead of letting it stick.
            (
                RootPhase::Running(running),
                WindowEvent::Focused(focused),
            ) => {
                let Running { sender,
                    turbo_toggle,
                    turbo_held,
                    rewinding,
                    paused,
                    pause_on_focus_loss,
                    focus_paused,
                    .. } = &mut **running;
                if !focused {
                    release_held_inputs(sender, *turbo_toggle, turbo_held, rewinding);
                    if *pause_on_focus_loss && !*paused {
                        *paused = true;
                        *focus_paused = true;
                        let _ = sender.send(GBEvent::SetPaused(true));
                    }
                } else if *focus_paused {
                    // Only undo our own pause, not one the user chose.
                    *focus_paused = false;
                    *paused = false;
                    let _ = sender.send(GBEvent::SetPaused(false));
                }
            }
            // Track modifier state so chords like Ctrl+R work.
            (
                RootPhase::Running(running),
                WindowEvent::ModifiersChanged(new_mods),
            ) => {
                running.modifiers = new_mods.state();
            }
            // ESC double-press logic: require two presses within ESC_DOUBLE_PRESS_MS to exit.
            (
                RootPhase::Running(running),
                WindowEvent::KeyboardInput {
                    event: keyevent, ..
                },
            ) => {
                let Running { sender,
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
                    toasts,
                    rom_path,
                    is_color,
                    dmg_palette_preset,
                    dmg_palette_custom,
                    .. } = &mut **running;
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
                    if matches!(state, winit::event::ElementState::Pressed)
                        && !keyevent.repeat
                        && keybindings.assign(kp, key_to_string(&logical.as_ref()))
                    {
                        // A refused (reserved) key leaves capture active.
                        *capturing = None;
                        let bindings_clone = keybindings.clone();
                        crate::config::update_config(|c| c.keybindings = bindings_clone);
                    }
                    return; // don't treat as game input
                }
                // OS key repeat would re-fire toggles (turbo, pause, mute),
                // spam save-state writes and turn a held Esc into a quit.
                if keyevent.repeat {
                    return;
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
                            toasts,
                        },
                    );
                    if outcome.screenshot {
                        let palette = (!*is_color)
                            .then(|| palette_for_preset(*dmg_palette_preset, dmg_palette_custom));
                        match latest_frame.as_deref() {
                            None => toasts.push("No frame to capture yet", ToastKind::Info),
                            Some(frame) => match save_screenshot(rom_path, frame, palette.as_ref()) {
                                Ok(path) => toasts.push(
                                    format!(
                                        "Screenshot saved: {}",
                                        path.file_name().unwrap_or_default().to_string_lossy()
                                    ),
                                    ToastKind::Success,
                                ),
                                Err(e) => {
                                    crate::log::warn(format!("screenshot failed: {e}"));
                                    toasts.push("Screenshot failed", ToastKind::Error)
                                }
                            },
                        }
                    }
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
                RootPhase::Running(running),
                WindowEvent::RedrawRequested,
            ) => {
                let Running { sender,
                    texture,
                    screen,
                    cgb_color_correction,
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
                    toasts,
                    integer_scaling,
                    auto_resume,
                    pause_on_focus_loss,
                    .. } = &mut **running;
                if !*running {
                    return;
                }
                drain_gui_events(ui_receiver, save_slots, toasts, &mut self.pending_rumble);
                // Deferred actions set inside the egui closure or below, applied after the borrow ends.
                let mut quit_requested = false;
                let mut reset_clicked = false;
                let mut open_recent: Option<PathBuf> = None;
                let mut new_scale: Option<u32> = None;
                let mut apply_fullscreen_now = false;
                let mut open_keybindings = false;
                let mut open_cheats = false;
                let recent_roms = self.recent_roms.clone();
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
                                        ui.separator();
                                        if ui
                                            .button("Undo Last Load")
                                            .on_hover_text("Return to the state from just before the last slot load")
                                            .clicked()
                                        {
                                            let _ = sender.send(GBEvent::UndoLoad);
                                            ui.close();
                                        }
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
                                    if ui.button("Cheats...").clicked() {
                                        open_cheats = true;
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
                                        for s in 1..=8 {
                                            let selected = cur_scale == s;
                                            if ui.radio(selected, format!("{}x", s)).clicked() {
                                                new_scale = Some(s);
                                            }
                                        }
                                    });
                                    ui.separator();
                                    ui.menu_button("Filter (Y)", |ui| {
                                        for f in crate::config::ScreenFilter::all() {
                                            if ui.radio(renderoptions.filter == *f, f.label()).clicked() {
                                                renderoptions.filter = *f;
                                                let f = *f;
                                                crate::config::update_config(|c| c.screen_filter = f);
                                            }
                                        }
                                    });
                                    if ui.checkbox(&mut renderoptions.lcd_grid, "LCD grid").changed() {
                                        let on = renderoptions.lcd_grid;
                                        crate::config::update_config(|c| c.lcd_grid = on);
                                    }
                                    if ui
                                        .add_enabled(is_color_ro, egui::Checkbox::new(cgb_color_correction, "CGB color correction"))
                                        .changed()
                                    {
                                        let on = *cgb_color_correction;
                                        let _ = sender.send(GBEvent::SetColorCorrection(on));
                                        crate::config::update_config(|c| c.cgb_color_correction = on);
                                    }
                                    if ui.checkbox(integer_scaling, "Integer scaling").changed() {
                                        let on = *integer_scaling;
                                        crate::config::update_config(|c| c.integer_scaling = on);
                                    }
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
                                                crate::config::update_config_debounced(|c| c.dmg_palette_custom = pal);
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
                                            crate::config::update_config_debounced(|c| c.volume = vol_now);
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
                                    if ui.checkbox(auto_resume, "Resume last session on load").changed() {
                                        let on = *auto_resume;
                                        crate::config::update_config(|c| c.auto_resume = on);
                                    }
                                    if ui.checkbox(pause_on_focus_loss, "Pause when in background").changed() {
                                        let on = *pause_on_focus_loss;
                                        crate::config::update_config(|c| c.pause_on_focus_loss = on);
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
                        toasts.draw(ctx);
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

                    // Black bars for the letterbox/pillarbox area.
                    target.clear_color(0.0, 0.0, 0.0, 1.0);
                    // Render the frame aspect-correct and centered in the game area.
                    if game_area_height > 0 {
                        let (left, bottom, w, h) =
                            fit_rect(target_w, game_area_height, *integer_scaling);
                        if w > 0 && h > 0 {
                            screen.draw(&mut target, texture, (left, bottom, w, h), renderoptions);
                        }
                    }

                    // Paint egui on top
                    egui_glium.paint(display, &mut target);
                    let _ = target.finish();
                    if egui_glium.egui_ctx().has_requested_repaint() {
                        window.request_redraw();
                    }

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
                    let fs = matches!(&self.phase, RootPhase::Running(running) if running.fullscreen);
                    if let Some(win) = &self.window {
                        apply_window_mode(win, s, fs);
                    }
                }
                if apply_fullscreen_now {
                    let fs = matches!(&self.phase, RootPhase::Running(running) if running.fullscreen);
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
                if open_cheats {
                    self.open_cheat_window(event_loop);
                }
            }
            // Drag-and-drop a ROM onto the running emulator: tear down and load the new one.
            (
                RootPhase::Running(_),
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
        self.maintain_audio();
        crate::config::flush_pending_config(false);
        // gilrs has no wakeup hook, so poll it on a timer: often while a pad
        // is connected and a game is running, rarely otherwise (hotplug).
        let gamepad_live = matches!(self.phase, RootPhase::Running(_))
            && self.gilrs.as_ref().is_some_and(|g| g.gamepads().next().is_some());
        let poll = Duration::from_millis(if gamepad_live { 4 } else { 250 });
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + poll));
        if let RootPhase::Running(running) = &mut self.phase
        {
            let Running { receiver,
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
            toasts,
            .. } = &mut **running;
            if !*running {
                return;
            }
            drain_gui_events(ui_receiver, save_slots, toasts, &mut self.pending_rumble);
            // Toasts animate even when no frames arrive (e.g. paused).
            if !toasts.is_empty() {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
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
        if let Some(on) = self.pending_rumble.take() {
            self.set_rumble(on);
        }
        // Gamepad-triggered Reset is queued as a pending action; window_event's
        // drain won't run until the next OS event, so drain here too.
        self.drain_pending_action();
    }
}

fn warn(message: &str) {
    crate::log::warn(message);
}

#[cfg(test)]
mod tests {
    use super::fit_rect;

    #[test]
    fn screenshot_writes_palette_mapped_png_next_to_rom() {
        let dir = std::env::temp_dir().join(format!("rgbe-shot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rom = dir.join("game.gb");
        // DMG frame: all lightest shade (255).
        let frame = vec![255u8; rust_gbe::SCREEN_W * rust_gbe::SCREEN_H * 3];
        let pal = crate::palette::DmgPalette { colors: [[1, 2, 3], [0; 3], [0; 3], [0; 3]] };
        let path = super::save_screenshot(&rom, &frame, Some(&pal)).unwrap();
        assert_eq!(path.parent().unwrap(), dir);
        assert!(path.file_name().unwrap().to_string_lossy().starts_with("game-"));

        let decoder = png::Decoder::new(std::fs::File::open(&path).unwrap());
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (160, 144));
        assert_eq!(&buf[..3], &[1, 2, 3], "DMG palette applied");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fit_rect_preserves_aspect_fractional() {
        // 1080/144 = 7.5 is the limiting scale → 1200x1080, centered horizontally.
        let (l, b, w, h) = fit_rect(1920, 1080, false);
        assert_eq!((w, h), (1200, 1080));
        assert_eq!((l, b), ((1920 - 1200) / 2, 0));
    }

    #[test]
    fn fit_rect_integer_floors_scale() {
        // floor(7.5) = 7 → 1120x1008, centered both ways.
        let (l, b, w, h) = fit_rect(1920, 1080, true);
        assert_eq!((w, h), (1120, 1008));
        assert_eq!((l, b), (400, 36));
    }

    #[test]
    fn fit_rect_exact_multiple_fills() {
        assert_eq!(fit_rect(480, 432, true), (0, 0, 480, 432));
    }

    #[test]
    fn fit_rect_tiny_and_zero_windows_do_not_panic() {
        // Below-native with integer scaling falls back to fractional shrink.
        let (_, _, w, h) = fit_rect(80, 72, true);
        assert!(w <= 80 && h <= 72 && w > 0 && h > 0);
        assert_eq!(fit_rect(0, 0, false), (0, 0, 0, 0));
    }
}
