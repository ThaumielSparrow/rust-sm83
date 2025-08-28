use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

mod gui;
mod audio;
mod emulator;
mod config;
mod input;

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
    let mut app = RootApp::new(DEFAULT_SCALE);
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    if let Err(e) = event_loop.run_app(&mut app) { eprintln!("Application error: {:?}", e); return EXITCODE_CPULOADFAILS; }
    app.exit_code
}
