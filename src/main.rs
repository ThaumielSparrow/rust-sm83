#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

mod gui;
mod audio;
mod emulator;
mod config;
mod input;
mod palette;

use std::path::PathBuf;

use gui::{RootApp, EXITCODE_CPULOADFAILS, EXITCODE_SUCCESS};

fn main() {
    let exit_status = real_main();
    if exit_status != EXITCODE_SUCCESS {
        std::process::exit(exit_status);
    }
}

fn real_main() -> i32 {
    const DEFAULT_SCALE: u32 = 3;
    let event_loop = winit::event_loop::EventLoop::new().unwrap();
    let pending_rom = std::env::args().nth(1).and_then(|a| {
        let p = PathBuf::from(&a);
        if p.is_file() { Some(p) } else { eprintln!("ROM path not found: {}", a); None }
    });
    let mut app = RootApp::new(DEFAULT_SCALE, pending_rom);
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    if let Err(e) = event_loop.run_app(&mut app) { eprintln!("Application error: {:?}", e); return EXITCODE_CPULOADFAILS; }
    app.exit_code
}
