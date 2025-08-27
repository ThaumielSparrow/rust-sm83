use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample};
use rust_gbe::device::Device;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

// Global setting for additional periodic auto-save functionality
static AUTO_SAVE_ENABLED: bool = false;

const EXITCODE_SUCCESS: i32 = 0;
const EXITCODE_CPULOADFAILS: i32 = 2;

#[derive(Default)]
struct RenderOptions {
    pub linear_interpolation: bool,
}

enum GBEvent {
    KeyUp(rust_gbe::KeypadKey),
    KeyDown(rust_gbe::KeypadKey),
    SpeedUp,
    SpeedDown,
    SaveState(u8),
    LoadState(u8),
}

// Unified state machine for ROM selection and emulator run to ensure a single EventLoop
enum RootPhase {
    Selecting { rom_path: String, browse_requested: bool },
    Running {
        texture: glium::texture::texture2d::Texture2d,
        sender: mpsc::Sender<GBEvent>,
        receiver: Receiver<Vec<u8>>,
        renderoptions: RenderOptions,
        running: bool,
        _audio: Option<cpal::Stream>,
    },
}

struct RootApp {
    window: Option<Arc<winit::window::Window>>,
    display: Option<glium::Display<glium::glutin::surface::WindowSurface>>,
    egui_glium: Option<egui_glium::EguiGlium>,
    phase: RootPhase,
    // future GUI settings could go here
    scale: u32,
    exit_code: i32,
}

impl RootApp {
    fn new(scale: u32) -> Self {
        let default_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_string_lossy().to_string())).unwrap_or_else(|| ".".to_string());
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
        if let Some((player, s)) = CpalPlayer::get() { cpu.enable_audio(Box::new(player) as Box<dyn rust_gbe::AudioPlayer>, true); audio_stream = Some(s);} else { warn("Audio disabled: no output device available"); }
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

// CLI argument parser removed; GUI-only application.

// Legacy separate ROM selector removed; handled inside RootApp now.

fn main() {
    let exit_status = real_main();
    if exit_status != EXITCODE_SUCCESS {
        std::process::exit(exit_status);
    }
}

fn real_main() -> i32 {
    const DEFAULT_SCALE: u32 = 3;
    let event_loop = winit::event_loop::EventLoop::new().unwrap();
    let mut app = RootApp::new(DEFAULT_SCALE);
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    if let Err(e) = event_loop.run_app(&mut app) { eprintln!("Application error: {:?}", e); return EXITCODE_CPULOADFAILS; }
    app.exit_code
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
    use glium::Surface;

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

fn construct_cpu_auto(filename: &str) -> Option<Box<Device>> {
    let rom_path = std::path::Path::new(filename);
    let save_state_path = rom_path.with_extension("state");
    let save_state_str = save_state_path.to_string_lossy().to_string();
    // Try CGB first, fallback to classic
    match Device::new_cgb(filename, false, Some(save_state_str.clone())) {
        Ok(cpu) => Some(Box::new(cpu)),
        Err(_) => match Device::new(filename, false, Some(save_state_str)) {
            Ok(cpu) => Some(Box::new(cpu)),
            Err(msg) => { warn(msg); None }
        }
    }
}

fn run_cpu(mut cpu: Box<Device>, sender: SyncSender<Vec<u8>>, receiver: Receiver<GBEvent>) {
    let periodic = timer_periodic(16);
    let mut limit_speed = true;

    let waitticks = (4194304f64 / 1000.0 * 16.0).round() as u32;
    let mut ticks = 0;
    let mut frame_count = 0;
    let mut last_ram_save_frame = 0;
    let mut ram_needs_save = false;

    'outer: loop {
        while ticks < waitticks {
            ticks += cpu.do_cycle();
            if cpu.check_and_reset_gpu_updated() {
                let data = cpu.get_gpu_data().to_vec();
                if let Err(TrySendError::Disconnected(..)) = sender.try_send(data) {
                    break 'outer;
                }
            }
        }

        ticks -= waitticks;
        frame_count += 1;

        // Always check for RAM updates and save immediately when game saves
        if cpu.check_and_reset_ram_updated() {
            // Immediate save when game writes to battery RAM (silent for better UX)
            if let Ok(_) = cpu.save_battery_ram_silent() {
                // Success - the save was triggered by actual game save activity
            }
            ram_needs_save = false; // Reset since we just saved
            last_ram_save_frame = frame_count;
        }

        // Additional periodic saving if auto-save is enabled
        if AUTO_SAVE_ENABLED && ram_needs_save && (frame_count - last_ram_save_frame) > 180 {
            if let Ok(_) = cpu.save_battery_ram_silent() {
                ram_needs_save = false;
            }
        }

        'recv: loop {
            match receiver.try_recv() {
                Ok(event) => match event {
                    GBEvent::KeyUp(key) => cpu.keyup(key),
                    GBEvent::KeyDown(key) => cpu.keydown(key),
                    GBEvent::SpeedUp => limit_speed = false,
                    GBEvent::SpeedDown => {
                        limit_speed = true;
                        cpu.sync_audio();
                    }
                    GBEvent::SaveState(slot) => {
                        println!("Attempting to save state to slot {}...", slot);
                        if let Err(e) = cpu.save_state_slot(slot) {
                            eprintln!("Failed to save state to slot {}: {}", slot, e);
                        }
                    }
                    GBEvent::LoadState(slot) => {
                        println!("Attempting to load state from slot {}...", slot);
                        if let Err(e) = cpu.load_state_slot(slot) {
                            eprintln!("Failed to load state from slot {}: {}", slot, e);
                        }
                    }
                    
                },
                Err(TryRecvError::Empty) => break 'recv,
                Err(TryRecvError::Disconnected) => break 'outer,
            }
        }

        if limit_speed {
            let _ = periodic.recv();
        }
    }
}

fn timer_periodic(ms: u64) -> Receiver<()> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        if tx.send(()).is_err() {
            break;
        }
    });
    rx
}

fn set_window_size(window: &winit::window::Window, scale: u32) {
    let _ = window.request_inner_size(winit::dpi::LogicalSize::<u32>::from((
        rust_gbe::SCREEN_W as u32 * scale,
        rust_gbe::SCREEN_H as u32 * scale,
    )));
}

struct CpalPlayer {
    buffer: Arc<Mutex<Vec<(f32, f32)>>>,
    sample_rate: u32,
}

impl CpalPlayer {
    fn get() -> Option<(CpalPlayer, cpal::Stream)> {
        let device = match cpal::default_host().default_output_device() {
            Some(e) => e,
            None => return None,
        };

        // We want a config with:
        // chanels = 2
        // SampleFormat F32
        // Rate at around 44100

        let wanted_samplerate = cpal::SampleRate(44100);
        let supported_configs = match device.supported_output_configs() {
            Ok(e) => e,
            Err(_) => return None,
        };
        let mut supported_config = None;
        for f in supported_configs {
            if f.channels() == 2 && f.sample_format() == cpal::SampleFormat::F32 {
                if f.min_sample_rate() <= wanted_samplerate
                    && wanted_samplerate <= f.max_sample_rate()
                {
                    supported_config = Some(f.with_sample_rate(wanted_samplerate));
                } else {
                    supported_config = Some(f.with_max_sample_rate());
                }
                break;
            }
        }
        if supported_config.is_none() {
            return None;
        }

        let selected_config = supported_config.unwrap();

        let sample_format = selected_config.sample_format();
        let config: cpal::StreamConfig = selected_config.into();

        let err_fn = |err| eprintln!("An error occurred on the output audio stream: {}", err);

        let shared_buffer = Arc::new(Mutex::new(Vec::new()));
        let stream_buffer = shared_buffer.clone();

        let player = CpalPlayer {
            buffer: shared_buffer,
            sample_rate: config.sample_rate.0,
        };

        let stream = match sample_format {
            cpal::SampleFormat::I8 => device.build_output_stream(
                &config,
                move |data: &mut [i8], _callback_info: &cpal::OutputCallbackInfo| {
                    cpal_thread(data, &stream_buffer)
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_output_stream(
                &config,
                move |data: &mut [i16], _callback_info: &cpal::OutputCallbackInfo| {
                    cpal_thread(data, &stream_buffer)
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::I32 => device.build_output_stream(
                &config,
                move |data: &mut [i32], _callback_info: &cpal::OutputCallbackInfo| {
                    cpal_thread(data, &stream_buffer)
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::I64 => device.build_output_stream(
                &config,
                move |data: &mut [i64], _callback_info: &cpal::OutputCallbackInfo| {
                    cpal_thread(data, &stream_buffer)
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::U8 => device.build_output_stream(
                &config,
                move |data: &mut [u8], _callback_info: &cpal::OutputCallbackInfo| {
                    cpal_thread(data, &stream_buffer)
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_output_stream(
                &config,
                move |data: &mut [u16], _callback_info: &cpal::OutputCallbackInfo| {
                    cpal_thread(data, &stream_buffer)
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::U32 => device.build_output_stream(
                &config,
                move |data: &mut [u32], _callback_info: &cpal::OutputCallbackInfo| {
                    cpal_thread(data, &stream_buffer)
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::U64 => device.build_output_stream(
                &config,
                move |data: &mut [u64], _callback_info: &cpal::OutputCallbackInfo| {
                    cpal_thread(data, &stream_buffer)
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _callback_info: &cpal::OutputCallbackInfo| {
                    cpal_thread(data, &stream_buffer)
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::F64 => device.build_output_stream(
                &config,
                move |data: &mut [f64], _callback_info: &cpal::OutputCallbackInfo| {
                    cpal_thread(data, &stream_buffer)
                },
                err_fn,
                None,
            ),
            sf => panic!("Unsupported sample format {}", sf),
        }
        .unwrap();

        stream.play().unwrap();

        Some((player, stream))
    }
}

fn cpal_thread<T: Sample + FromSample<f32>>(
    outbuffer: &mut [T],
    audio_buffer: &Arc<Mutex<Vec<(f32, f32)>>>,
) {
    let mut inbuffer = audio_buffer.lock().unwrap();
    let outlen = ::std::cmp::min(outbuffer.len() / 2, inbuffer.len());
    for (i, (in_l, in_r)) in inbuffer.drain(..outlen).enumerate() {
        outbuffer[i * 2] = T::from_sample(in_l);
        outbuffer[i * 2 + 1] = T::from_sample(in_r);
    }
}

impl rust_gbe::AudioPlayer for CpalPlayer {
    fn play(&mut self, buf_left: &[f32], buf_right: &[f32]) {
        debug_assert!(buf_left.len() == buf_right.len());

        let mut buffer = self.buffer.lock().unwrap();

        for (l, r) in buf_left.iter().zip(buf_right) {
            if buffer.len() > self.sample_rate as usize {
                // Do not fill the buffer with more than 1 second of data
                // This speeds up the resync after the turning on and off the speed limiter
                return;
            }
            buffer.push((*l, *r));
        }
    }

    fn samples_rate(&self) -> u32 {
        self.sample_rate
    }

    fn underflowed(&self) -> bool {
        (*self.buffer.lock().unwrap()).len() == 0
    }
}

// (Removed legacy NullAudioPlayer and test-mode / stdin helpers)
