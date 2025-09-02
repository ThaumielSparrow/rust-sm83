use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread;

use cpal::Stream;
use glium::Surface;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

pub const EXITCODE_SUCCESS: i32 = 0;
pub const EXITCODE_CPULOADFAILS: i32 = 2;

#[derive(Default)]
pub struct RenderOptions {
    pub linear_interpolation: bool,
}

use crate::emulator::{GBEvent, construct_cpu_auto, run_cpu};
use crate::audio::init_audio;
use crate::config::{Config, KeyBindings, config_path, binding_value, TurboSetting};
use crate::input::is_reserved_key_name;

// Unified state machine for ROM selection and emulator run to ensure a single EventLoop
enum RootPhase {
    Selecting { rom_path: String, browse_requested: bool },
    Running {
        texture: glium::texture::texture2d::Texture2d,
        sender: mpsc::Sender<GBEvent>,
        receiver: Receiver<Arc<Vec<u8>>>,
        renderoptions: RenderOptions,
        running: bool,
        keybindings: KeyBindings,
        capturing: Option<rust_gbe::KeypadKey>,
        _audio: Option<Stream>,
        show_keybindings_window: bool,
        turbo_toggle: bool,
        turbo_held: bool,
        turbo_setting: TurboSetting,
    },
}

pub struct RootApp {
    window: Option<Arc<winit::window::Window>>,
    display: Option<glium::Display<glium::glutin::surface::WindowSurface>>,
    egui_glium: Option<egui_glium::EguiGlium>,
    phase: RootPhase,
    // future GUI settings could go here
    scale: u32,
    pub exit_code: i32,
}

impl RootApp {
    pub fn new(scale: u32) -> Self {
        let default_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_string_lossy().to_string()))
            .unwrap_or_else(|| ".".to_string());
        let _cfg = Config::load(&config_path());
        RootApp {
            window: None,
            display: None,
            egui_glium: None,
            phase: RootPhase::Selecting { rom_path: default_dir, browse_requested: false },
            scale,
            exit_code: EXITCODE_SUCCESS,
        }
    }

    fn start_game(&mut self, filename: String) {
        // Always run in (CGB-capable) mode; attempt CGB first, fallback to classic if needed.
        let cpu = construct_cpu_auto(&filename);
        let mut cpu = match cpu { Some(c) => c, None => { self.exit_code = EXITCODE_CPULOADFAILS; return; } };
        // Enable audio by default; if device fails, continue silently.
        let mut audio_stream = None;
        if let Some((player, s)) = init_audio() {
            cpu.enable_audio(player, true); audio_stream = Some(s);
        } else { 
            warn("Audio disabled: no output device available");
        }
        let _ = cpu.romname();
        let (sender, recv_events) = mpsc::channel();
    let (frame_sender, frame_receiver) = mpsc::sync_channel(1);
        let frame_sender_clone = frame_sender.clone();
        thread::spawn(move || run_cpu(cpu, frame_sender_clone, recv_events));
        if let Some(display) = &self.display {
            let texture = glium::texture::texture2d::Texture2d::empty_with_format(
                display,
                glium::texture::UncompressedFloatFormat::U8U8U8,
                glium::texture::MipmapsOption::NoMipmap,
                rust_gbe::SCREEN_W as u32,
                rust_gbe::SCREEN_H as u32,
            ).unwrap();
            let cfg = Config::load(&config_path());
            let initial_scale = cfg.scale;
            if let Some(win) = &self.window {
                set_window_size(win, initial_scale);
            }
            self.scale = initial_scale;
            self.phase = RootPhase::Running { texture, sender, receiver: frame_receiver, renderoptions: RenderOptions::default(), running: true, keybindings: cfg.keybindings, capturing: None, _audio: audio_stream, show_keybindings_window: false, turbo_toggle: false, turbo_held: false, turbo_setting: cfg.turbo };
            if let RootPhase::Running { sender, .. } = &self.phase { let _ = sender.send(GBEvent::UpdateTurbo(cfg.turbo)); }
            // Now that we've transitioned to Running, resize window to game resolution * scale.
            if let Some(win) = &self.window {
                set_window_size(win, self.scale);
            }
        } else {
            self.exit_code = EXITCODE_CPULOADFAILS;
        }
    }
}

impl ApplicationHandler for RootApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let (window, display) = glium::backend::glutin::SimpleWindowBuilder::new()
                .with_title("Game Boy Emulator")
                .with_inner_size(600, 220)
                .build(event_loop);
            let egui_glium = egui_glium::EguiGlium::new(egui::ViewportId::ROOT, &display, &window, &event_loop);
            self.egui_glium = Some(egui_glium);
            self.display = Some(display);
            self.window = Some(Arc::new(window));
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
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
            if resp.consumed { return; }
        }

        match (&mut self.phase, event) {
            (_, WindowEvent::CloseRequested) => { event_loop.exit(); },
            (RootPhase::Selecting { rom_path, browse_requested }, WindowEvent::RedrawRequested) => {
                if *browse_requested { *browse_requested = false; if let Some(p) = rfd::FileDialog::new().add_filter("Game Boy ROMs", &["gb","gbc"]).add_filter("All files", &["*"]).set_directory(&rom_path).pick_file() { *rom_path = p.to_string_lossy().to_string(); } }
                let mut launch_filename: Option<String> = None;
                let mut quit_requested = false;
                if let (Some(window), Some(display), Some(egui_glium)) = (&self.window, &self.display, &mut self.egui_glium) {
                    egui_glium.run(window, |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            ui.heading("Game Boy Emulator");
                            ui.add_space(8.0);
                            ui.label("Select a ROM file to load:");
                            ui.horizontal(|ui| {
                                ui.label("ROM:");
                                ui.add(egui::TextEdit::singleline(rom_path).desired_width(340.0));
                                if ui.button("Browse").clicked() { *browse_requested = true; }
                            });
                            ui.add_space(8.0);
                            if ui.button("Load ROM").clicked() { if std::path::Path::new(&rom_path).exists() { launch_filename = Some(rom_path.clone()); } }
                            if ui.button("Quit").clicked() { quit_requested = true; }
                            ui.add_space(6.0);
                            if rom_path.is_empty() { ui.colored_label(egui::Color32::GRAY, "Enter a path to a .gb/.gbc file"); }
                            else if !std::path::Path::new(&rom_path).exists() { ui.colored_label(egui::Color32::RED, "File does not exist"); }
                            else { ui.colored_label(egui::Color32::GREEN, "Path OK"); }
                        });
                    });
                    // Paint after UI
                    let mut target = display.draw();
                    target.clear_color(0.1,0.1,0.1,1.0);
                    egui_glium.paint(display, &mut target); let _ = target.finish();
                }
                if let Some(f) = launch_filename { self.start_game(f); }
                if quit_requested { self.exit_code = EXITCODE_CPULOADFAILS; event_loop.exit(); }
            }
            (RootPhase::Running { sender, renderoptions, running, keybindings, capturing, show_keybindings_window, turbo_toggle, turbo_held, turbo_setting, .. }, WindowEvent::KeyboardInput { event: keyevent, .. }) => {
                let state = keyevent.state;
                let logical = keyevent.logical_key.clone();
                if let Some(kp) = *capturing {
                    // Capturing mode: ESC cancels, any other key assigns.
                    if let Key::Named(NamedKey::Escape) = logical.as_ref() { *capturing = None; return; }
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
                        // Persist current turbo setting along with keybindings
                        let cfg = Config {
                            keybindings: keybindings.clone(),
                            scale: self.scale,
                            turbo: *turbo_setting
                        };
                        cfg.save(&config_path());
                    }
                    return; // don't treat as game input
                }
                if let Some(action) = crate::input::system_action_for(&logical.as_ref(), state) {
                    use crate::input::SystemAction;
                    match action {
                        SystemAction::SaveState(s)=>{ let _=sender.send(GBEvent::SaveState(s)); },
                        SystemAction::LoadState(s)=>{ let _=sender.send(GBEvent::LoadState(s)); },
                        SystemAction::TurboHold(press)=>{
                            if press {
                                if !*turbo_toggle && !*turbo_held {
                                    let _=sender.send(GBEvent::SpeedUp);
                                } *turbo_held=true;
                            } else { 
                                *turbo_held=false;
                                if !*turbo_toggle {
                                    let _=sender.send(GBEvent::SpeedDown);
                                }
                            }
                        },
                        SystemAction::TurboToggle=>{ *turbo_toggle=! *turbo_toggle; if *turbo_toggle { if !*turbo_held { let _=sender.send(GBEvent::SpeedUp);} } else if !*turbo_held { let _=sender.send(GBEvent::SpeedDown);} },
                        SystemAction::ToggleInterpolation=>{ renderoptions.linear_interpolation = !renderoptions.linear_interpolation; },
                    }
                    return;
                }
                match (state, logical.as_ref()) {
                    // Escape: if keybindings window open, close it; else exit emulator
                    (Pressed, Key::Named(NamedKey::Escape)) => {
                        if *show_keybindings_window { *show_keybindings_window = false; }
                        else { *running = false; event_loop.exit(); }
                    },
                    (Pressed, wkey) => { if let Some(k)=dynamic_winit_to_keypad(wkey, keybindings) { let _=sender.send(GBEvent::KeyDown(k)); } },
                    (Released, wkey) => { if let Some(k)=dynamic_winit_to_keypad(wkey, keybindings) { let _=sender.send(GBEvent::KeyUp(k)); } },
                }
            }
            (RootPhase::Running { sender, texture, receiver, renderoptions, running, keybindings, capturing, show_keybindings_window, turbo_toggle, turbo_setting, .. }, WindowEvent::RedrawRequested) => {
                if !*running { return; }
                if let (Some(display), Some(window), Some(egui_glium)) = (&self.display, &self.window, &mut self.egui_glium) {
                    // Get the menu bar height first
                    let mut menu_bar_height = 0.0;
                    egui_glium.run(window, |ctx| {
                        let top_panel = egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
                            egui::menu::bar(ui, |ui| {
                                ui.menu_button("File", |ui| {
                                    ui.menu_button("Save State", |ui| {
                                        for i in 1..=4 { if ui.button(format!("Slot {}", i)).clicked() { let _=sender.send(GBEvent::SaveState(i)); ui.close_menu(); } }
                                    });
                                    ui.menu_button("Load State", |ui| {
                                        for i in 1..=4 { if ui.button(format!("Slot {}", i)).clicked() { let _=sender.send(GBEvent::LoadState(i)); ui.close_menu(); } }
                                    });
                                    ui.separator();
                                    if ui.button("Quit").clicked() { *running=false; ui.close_menu(); }
                                });
                                ui.menu_button("Options", |ui| {
                                    ui.menu_button("Scale", |ui| {
                                        for s in 1..=4 { let selected = self.scale == s; if ui.radio(selected, format!("{}x", s)).clicked() { self.scale = s; set_window_size(window, s); let cfg = Config { keybindings: keybindings.clone(), scale: self.scale, turbo: *turbo_setting }; cfg.save(&config_path()); } }
                                    });
                                    ui.menu_button("Turbo Speed", |ui| {
                                        for ts in TurboSetting::all() {
                                            let selected = *turbo_setting == *ts;
                                            if ui.radio(selected, ts.label()).clicked() {
                                                *turbo_setting = *ts;
                                                let cfg = Config { keybindings: keybindings.clone(), scale: self.scale, turbo: *turbo_setting }; cfg.save(&config_path());
                                                let _ = sender.send(GBEvent::UpdateTurbo(*ts));
                                            }
                                        }
                                    });
                                    ui.checkbox(turbo_toggle, "Turbo Enabled (T)");
                                    if ui.button("Keybindings...").clicked() { *show_keybindings_window = true; }
                                });
                            });
                        });
                        menu_bar_height = top_panel.response.rect.height();
                        
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
                    
                    // Draw game texture with offset for menu bar
                    use glium::Surface; let mut target = display.draw();
                    let (target_w, target_h) = target.get_dimensions();
                    
                    // Calculate menu bar height in pixels
                    let menu_bar_height_pixels = (menu_bar_height * window.scale_factor() as f32) as u32;
                    let game_area_height = target_h.saturating_sub(menu_bar_height_pixels);
                    
                    // Render game texture offset downward by menu bar height
                    if game_area_height > 0 {
                        let interpolation_type = if renderoptions.linear_interpolation { glium::uniforms::MagnifySamplerFilter::Linear } else { glium::uniforms::MagnifySamplerFilter::Nearest };
                        texture.as_surface().blit_whole_color_to(&target, &glium::BlitTarget { 
                            left: 0, 
                            bottom: game_area_height,  // Position at bottom of available area
                            width: target_w as i32, 
                            height: -(game_area_height as i32)  // Negative height to flip Y
                        }, interpolation_type);
                    }
                    
                    // Paint egui on top
                    egui_glium.paint(display, &mut target); 
                    let _ = target.finish();
                }
                // Drain any queued frames and upload
                loop { match receiver.try_recv() { Ok(data)=>{ upload_screen(texture, &data); }, Err(TryRecvError::Empty)=>break, Err(TryRecvError::Disconnected)=>{ *running=false; event_loop.exit(); break; } } }
            }
            _ => { if let Some(w) = &self.window { w.request_redraw(); } }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let RootPhase::Running { receiver, texture, running, .. } = &mut self.phase {
            if !*running { return; }
            match receiver.try_recv() {
                Ok(data) => { upload_screen(texture, &data); if let Some(w) = &self.window { w.request_redraw(); } },
                Err(TryRecvError::Empty) => {},
                Err(TryRecvError::Disconnected) => { *running = false; event_loop.exit(); },
            }
        }
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
        glium::Rect { left: 0, bottom: 0, width: rust_gbe::SCREEN_W as u32, height: rust_gbe::SCREEN_H as u32 },
        rawimage2d,
    );
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

// Dynamic mapping using current keybindings
fn dynamic_winit_to_keypad(key: winit::keyboard::Key<&str>, bindings: &KeyBindings) -> Option<rust_gbe::KeypadKey> {
    use winit::keyboard::{Key, NamedKey};
    match key {
        Key::Character(c) => {
            let upc = c.to_uppercase();
            if upc == bindings.a { Some(rust_gbe::KeypadKey::A) }
            else if upc == bindings.b { Some(rust_gbe::KeypadKey::B) }
            else if upc == bindings.start { Some(rust_gbe::KeypadKey::Start) }
            else if upc == bindings.select { Some(rust_gbe::KeypadKey::Select) }
            else if upc == bindings.up { Some(rust_gbe::KeypadKey::Up) }
            else if upc == bindings.down { Some(rust_gbe::KeypadKey::Down) }
            else if upc == bindings.left { Some(rust_gbe::KeypadKey::Left) }
            else if upc == bindings.right { Some(rust_gbe::KeypadKey::Right) }
            else { None }
        }
        Key::Named(named) => match named {
            NamedKey::ArrowUp if bindings.up == "ArrowUp" => Some(rust_gbe::KeypadKey::Up),
            NamedKey::ArrowDown if bindings.down == "ArrowDown" => Some(rust_gbe::KeypadKey::Down),
            NamedKey::ArrowLeft if bindings.left == "ArrowLeft" => Some(rust_gbe::KeypadKey::Left),
            NamedKey::ArrowRight if bindings.right == "ArrowRight" => Some(rust_gbe::KeypadKey::Right),
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
        (Some(A), A)|(Some(B), B)|(Some(Start), Start)|(Some(Select), Select)|
        (Some(Up), Up)|(Some(Down), Down)|(Some(Left), Left)|(Some(Right), Right) => true,
        _ => false,
    }
}
