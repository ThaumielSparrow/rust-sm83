#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

mod gui;
mod audio;
mod cheats;
mod emulator;
mod config;
mod input;
mod gamepad;
mod palette;
mod toast;
mod log;

use std::path::PathBuf;

use gui::{RootApp, EXITCODE_CPULOADFAILS, EXITCODE_SUCCESS};

fn main() {
    let exit_status = real_main();
    if exit_status != EXITCODE_SUCCESS {
        std::process::exit(exit_status);
    }
}

/// Release builds have no console, so append panics to crash.log (next to
/// config.json) before running the default hook.
fn install_panic_log() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        let path = config::config_path().with_file_name("crash.log");
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let thread = std::thread::current();
        let entry = format!(
            "[unix {}] {} v{} thread '{}': {}\n",
            secs,
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
            thread.name().unwrap_or("<unnamed>"),
            info
        );
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(entry.as_bytes()));
        default_hook(info);
    }));
}

fn real_main() -> i32 {
    log::init();
    install_panic_log();
    const DEFAULT_SCALE: u32 = 3;
    let event_loop = winit::event_loop::EventLoop::<gui::UserEvent>::with_user_event()
        .build()
        .unwrap();
    let pending_rom = std::env::args().nth(1).and_then(|a| {
        let p = PathBuf::from(&a);
        if p.is_file() { Some(p) } else { crate::log::warn(format!("ROM path not found: {}", a)); None }
    });
    let mut app = RootApp::new(DEFAULT_SCALE, pending_rom, event_loop.create_proxy());
    // Sleep between events; the emulator thread wakes the loop per frame.
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        event_loop.run_app(&mut app)
    }));
    match result {
        Ok(Ok(())) => app.exit_code,
        Ok(Err(e)) => {
            crate::log::warn(format!("Application error: {:?}", e));
            app.stop_emulator();
            EXITCODE_CPULOADFAILS
        }
        Err(panic) => {
            // GUI-thread panic: still let the emulator thread flush battery
            // RAM and the auto-save before the process goes away.
            app.stop_emulator();
            std::panic::resume_unwind(panic);
        }
    }
}
