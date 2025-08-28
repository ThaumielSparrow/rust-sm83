use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread;

use cpal::Stream;
use glium::Surface; // bring trait methods (get_dimensions, blit_whole_color_to) into scope
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

// Re-export for main.rs
pub const EXITCODE_SUCCESS: i32 = 0;
pub const EXITCODE_CPULOADFAILS: i32 = 2;

// (auto-save constant moved to emulator module)

#[derive(Default)]
pub struct RenderOptions {
    pub linear_interpolation: bool,
}

use crate::emulator::{GBEvent, construct_cpu_auto, run_cpu};
use crate::audio::init_audio;

// Unified state machine for ROM selection and emulator run to ensure a single EventLoop
enum RootPhase {
    Selecting { rom_path: String, browse_requested: bool },
    Running {
        texture: glium::texture::texture2d::Texture2d,
        sender: mpsc::Sender<GBEvent>,
        receiver: Receiver<Vec<u8>>,
        renderoptions: RenderOptions,
        running: bool,
    _audio: Option<Stream>,
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
    if let Some((player, s)) = init_audio() { cpu.enable_audio(player, true); audio_stream = Some(s);} else { warn("Audio disabled: no output device available"); }
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
            self.phase = RootPhase::Running { texture, sender, receiver: frame_receiver, renderoptions: RenderOptions::default(), running: true, _audio: audio_stream };
            // Now that we've transitioned to Running, resize window to game resolution * scale.
            if let Some(win) = &self.window { set_window_size(win, self.scale); }
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
            if let Some(w) = &self.window { w.request_redraw(); }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
        use winit::event::ElementState::{Pressed, Released};
        use winit::keyboard::{Key, NamedKey};

        // Pass events to egui only when selecting
        if matches!(self.phase, RootPhase::Selecting { .. }) {
            if let Some(egui_glium) = &mut self.egui_glium { let resp = egui_glium.on_event(self.window.as_ref().unwrap(), &event); if resp.repaint { if let Some(w) = &self.window { w.request_redraw(); } } if resp.consumed { return; } }
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
                    use glium::Surface; let mut target = display.draw(); target.clear_color(0.1,0.1,0.1,1.0); egui_glium.paint(display, &mut target); let _ = target.finish();
                }
                if let Some(f) = launch_filename { self.start_game(f); }
                if quit_requested { self.exit_code = EXITCODE_CPULOADFAILS; event_loop.exit(); }
            }
            (RootPhase::Running { sender, renderoptions, running, .. }, WindowEvent::KeyboardInput { event: keyevent, .. }) => {
                match (keyevent.state, keyevent.logical_key.as_ref()) {
                    (Pressed, Key::Named(NamedKey::Escape)) => { *running = false; event_loop.exit(); },
                    (Pressed, Key::Character("1")) => { if let Some(w) = &self.window { set_window_size(w, 1); } },
                    (Pressed, Key::Character("r"|"R")) => { if let Some(w) = &self.window { set_window_size(w, self.scale); } },
                    (Pressed, Key::Named(NamedKey::Shift)) => { let _ = sender.send(GBEvent::SpeedUp); },
                    (Released, Key::Named(NamedKey::Shift)) => { let _ = sender.send(GBEvent::SpeedDown); },
                    (Pressed, Key::Character("t"|"T")) => { renderoptions.linear_interpolation = !renderoptions.linear_interpolation; },
                    (Pressed, Key::Named(NamedKey::F1)) => { let _ = sender.send(GBEvent::SaveState(1)); },
                    (Pressed, Key::Named(NamedKey::F2)) => { let _ = sender.send(GBEvent::SaveState(2)); },
                    (Pressed, Key::Named(NamedKey::F3)) => { let _ = sender.send(GBEvent::SaveState(3)); },
                    (Pressed, Key::Named(NamedKey::F4)) => { let _ = sender.send(GBEvent::SaveState(4)); },
                    (Pressed, Key::Named(NamedKey::F5)) => { let _ = sender.send(GBEvent::LoadState(1)); },
                    (Pressed, Key::Named(NamedKey::F6)) => { let _ = sender.send(GBEvent::LoadState(2)); },
                    (Pressed, Key::Named(NamedKey::F7)) => { let _ = sender.send(GBEvent::LoadState(3)); },
                    (Pressed, Key::Named(NamedKey::F8)) => { let _ = sender.send(GBEvent::LoadState(4)); },
                    (Pressed, wkey) => { if let Some(k)=winit_to_keypad(wkey) { let _=sender.send(GBEvent::KeyDown(k)); } },
                    (Released, wkey) => { if let Some(k)=winit_to_keypad(wkey) { let _=sender.send(GBEvent::KeyUp(k)); } },
                }
            }
            _ => { if let Some(w) = &self.window { w.request_redraw(); } }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let RootPhase::Running { receiver, texture, renderoptions, running, .. } = &mut self.phase {
            if !*running { return; }
            match receiver.try_recv() {
                Ok(data) => { if let Some(display) = &self.display { recalculate_screen(display, texture, &data, renderoptions); } },
                Err(TryRecvError::Empty) => {},
                Err(TryRecvError::Disconnected) => { *running = false; event_loop.exit(); },
            }
        }
    }
}

fn winit_to_keypad(key: winit::keyboard::Key<&str>) -> Option<rust_gbe::KeypadKey> {
    use winit::keyboard::{Key, NamedKey};
    match key {
        Key::Character("Z" | "z") => Some(rust_gbe::KeypadKey::A),
        Key::Character("X" | "x") => Some(rust_gbe::KeypadKey::B),
        Key::Named(NamedKey::ArrowUp) => Some(rust_gbe::KeypadKey::Up),
        Key::Named(NamedKey::ArrowDown) => Some(rust_gbe::KeypadKey::Down),
        Key::Named(NamedKey::ArrowLeft) => Some(rust_gbe::KeypadKey::Left),
        Key::Named(NamedKey::ArrowRight) => Some(rust_gbe::KeypadKey::Right),
        Key::Named(NamedKey::Space) => Some(rust_gbe::KeypadKey::Select),
        Key::Named(NamedKey::Enter) => Some(rust_gbe::KeypadKey::Start),
        _ => None,
    }
}

fn recalculate_screen<
    T: glium::glutin::surface::SurfaceTypeTrait + glium::glutin::surface::ResizeableSurface + 'static,
>(
    display: &glium::Display<T>,
    texture: &mut glium::texture::texture2d::Texture2d,
    datavec: &[u8],
    renderoptions: &RenderOptions,
) {
    let interpolation_type = if renderoptions.linear_interpolation {
        glium::uniforms::MagnifySamplerFilter::Linear
    } else {
        glium::uniforms::MagnifySamplerFilter::Nearest
    };

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

    // We use a custom BlitTarget to transform OpenGL coordinates to row-column coordinates
    let target = display.draw();
    let (target_w, target_h) = target.get_dimensions();
    texture.as_surface().blit_whole_color_to(
        &target,
        &glium::BlitTarget {
            left: 0,
            bottom: target_h,
            width: target_w as i32,
            height: -(target_h as i32),
        },
        interpolation_type,
    );
    target.finish().unwrap();
}

fn warn(message: &str) {
    eprintln!("{}", message);
}

fn set_window_size(window: &winit::window::Window, scale: u32) {
    let _ = window.request_inner_size(winit::dpi::LogicalSize::<u32>::from((
        rust_gbe::SCREEN_W as u32 * scale,
        rust_gbe::SCREEN_H as u32 * scale,
    )));
}

// (Audio backend & run loop moved to modules `audio` and `emulator`)
