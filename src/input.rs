//! Centralized definitions for system (non-rebindable) key actions and helpers.
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// System actions triggered directly by keys (not remapped by user)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemAction {
    SaveState(u8),
    LoadState(u8),
    TurboHold(bool), // true=press, false=release
    TurboToggle,
    ToggleInterpolation,
    TogglePause,
    Reset,
    ToggleFullscreen,
    ToggleMute,
    ToggleFpsOverlay,
}

/// Static mapping of (winit Key + modifiers) to SystemAction. Modifiers matter for
/// chords like Ctrl+R; plain `R` is left rebindable.
pub fn system_action_for(
    key: &Key<&str>,
    state: winit::event::ElementState,
    modifiers: ModifiersState,
) -> Option<SystemAction> {
    use winit::event::ElementState::{Pressed, Released};
    use SystemAction::*;
    let ctrl = modifiers.control_key();
    match (state, key) {
        // Ctrl+R: reset emulator
        (Pressed, Key::Character("r" | "R")) if ctrl => Some(Reset),
        (Pressed, Key::Named(NamedKey::F1)) => Some(SaveState(1)),
        (Pressed, Key::Named(NamedKey::F2)) => Some(SaveState(2)),
        (Pressed, Key::Named(NamedKey::F3)) => Some(SaveState(3)),
        (Pressed, Key::Named(NamedKey::F4)) => Some(SaveState(4)),
        (Pressed, Key::Named(NamedKey::F5)) => Some(LoadState(1)),
        (Pressed, Key::Named(NamedKey::F6)) => Some(LoadState(2)),
        (Pressed, Key::Named(NamedKey::F7)) => Some(LoadState(3)),
        (Pressed, Key::Named(NamedKey::F8)) => Some(LoadState(4)),
        (Pressed, Key::Named(NamedKey::F9)) => Some(ToggleFpsOverlay),
        (Pressed, Key::Named(NamedKey::F11)) => Some(ToggleFullscreen),
        (Pressed, Key::Named(NamedKey::Shift)) => Some(TurboHold(true)),
        (Released, Key::Named(NamedKey::Shift)) => Some(TurboHold(false)),
        (Pressed, Key::Character("t" | "T")) => Some(TurboToggle),
        (Pressed, Key::Character("y" | "Y")) => Some(ToggleInterpolation),
        (Pressed, Key::Character("p" | "P")) => Some(TogglePause),
        (Pressed, Key::Character("m" | "M")) => Some(ToggleMute),
        _ => None,
    }
}

/// Keys reserved for emulator system actions (not allowed for gamepad bindings).
pub const RESERVED_KEYS: &[&str] = &[
    "F1","F2","F3","F4","F5","F6","F7","F8", // save/load slots 1-4
    "F9","F11",                              // FPS overlay, fullscreen
    "Shift","T","Y","P","M",                 // turbo hold/toggle, interpolation, pause, mute
];

pub fn is_reserved_key_name(name: &str) -> bool {
    // Case-insensitive for letters
    let upper = name.to_uppercase();
    RESERVED_KEYS.iter().any(|k| k.eq_ignore_ascii_case(&upper))
}
