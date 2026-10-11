pub use crate::gpu::{SCREEN_H, SCREEN_W};
pub use crate::keypad::KeypadKey;
pub use crate::apu::AudioPlayer;
pub use crate::mbc::CHECKSUM_INVALID;

pub mod device;

mod cpu;
mod gbmode;
mod gpu;
mod keypad;
mod mbc;
mod mmu;
mod register;
mod serial;
mod apu;
mod timer;

#[cfg(test)]
mod test_roms;

pub type StrResult<T> = Result<T, &'static str>;
