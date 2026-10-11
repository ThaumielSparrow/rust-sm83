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
    RewindHold(bool), // true=press, false=release
    RewindStep,
    Screenshot,
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
    let bank = if ctrl { 4 } else { 0 };
    match (state, key) {
        // Ctrl+R: reset emulator
        (Pressed, Key::Character("r" | "R")) if ctrl => Some(Reset),
        // F1-F4 save / F5-F8 load slots 1-4; with Ctrl, slots 5-8.
        (Pressed, Key::Named(NamedKey::F1)) => Some(SaveState(1 + bank)),
        (Pressed, Key::Named(NamedKey::F2)) => Some(SaveState(2 + bank)),
        (Pressed, Key::Named(NamedKey::F3)) => Some(SaveState(3 + bank)),
        (Pressed, Key::Named(NamedKey::F4)) => Some(SaveState(4 + bank)),
        (Pressed, Key::Named(NamedKey::F5)) => Some(LoadState(1 + bank)),
        (Pressed, Key::Named(NamedKey::F6)) => Some(LoadState(2 + bank)),
        (Pressed, Key::Named(NamedKey::F7)) => Some(LoadState(3 + bank)),
        (Pressed, Key::Named(NamedKey::F8)) => Some(LoadState(4 + bank)),
        (Pressed, Key::Named(NamedKey::F9)) => Some(ToggleFpsOverlay),
        (Pressed, Key::Named(NamedKey::F11)) => Some(ToggleFullscreen),
        (Pressed, Key::Named(NamedKey::F12)) => Some(Screenshot),
        (Pressed, Key::Named(NamedKey::Shift)) => Some(TurboHold(true)),
        (Released, Key::Named(NamedKey::Shift)) => Some(TurboHold(false)),
        (Pressed, Key::Named(NamedKey::Backspace)) => Some(RewindHold(true)),
        (Released, Key::Named(NamedKey::Backspace)) => Some(RewindHold(false)),
        (Pressed, Key::Character("\\")) => Some(RewindStep),
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
    "F9","F11","F12",                        // FPS overlay, fullscreen, screenshot
    "Shift","T","Y","P","M",                 // turbo hold/toggle, interpolation, pause, mute
    "Backspace","\\",                        // rewind hold / step back
];

/// Bindings starting with this name a physical key position (winit `KeyCode`),
/// so they work the same on every keyboard layout (e.g. AZERTY). Bindings
/// without it are logical key names, as written by older versions.
pub const PHYSICAL_PREFIX: &str = "Code:";

/// The binding string for a physical key, if it has a known code.
pub fn physical_binding(key: &winit::keyboard::PhysicalKey) -> Option<String> {
    match key {
        winit::keyboard::PhysicalKey::Code(code) => Some(format!("{PHYSICAL_PREFIX}{code:?}")),
        _ => None,
    }
}

/// Short display name for a binding: "Code:KeyX" -> "X", "Code:Digit1" ->
/// "1", "Code:ShiftLeft" -> "Shift"; logical names are shown as stored.
pub fn binding_label(binding: &str) -> String {
    let Some(code) = binding.strip_prefix(PHYSICAL_PREFIX) else {
        return binding.to_string();
    };
    if let Some(letter) = code.strip_prefix("Key") {
        return letter.to_string();
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        return digit.to_string();
    }
    match code {
        "ShiftLeft" | "ShiftRight" => "Shift".into(),
        "Backslash" => "\\".into(),
        other => other.to_string(),
    }
}

/// Whether `binding` is triggered by a key with this logical name and
/// physical binding string.
pub fn binding_matches(binding: &str, logical: &str, physical: Option<&str>) -> bool {
    if binding.starts_with(PHYSICAL_PREFIX) {
        physical == Some(binding)
    } else {
        binding.eq_ignore_ascii_case(logical)
    }
}

pub fn is_reserved_key_name(name: &str) -> bool {
    // Case-insensitive for letters
    let upper = name.to_uppercase();
    RESERVED_KEYS.iter().any(|k| k.eq_ignore_ascii_case(&upper))
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::event::ElementState::{Pressed, Released};
    use winit::keyboard::{Key, NamedKey};

    #[test]
    fn physical_bindings_match_by_position_and_label_readably() {
        use winit::keyboard::{KeyCode, PhysicalKey};
        let x = physical_binding(&PhysicalKey::Code(KeyCode::KeyX)).unwrap();
        assert_eq!(x, "Code:KeyX");
        assert_eq!(binding_label(&x), "X");
        assert_eq!(binding_label("Code:ShiftLeft"), "Shift");
        assert_eq!(binding_label("Enter"), "Enter");
        // Physical binding: matches the position whatever the layout types.
        assert!(binding_matches(&x, "Q", Some("Code:KeyX")));
        assert!(!binding_matches(&x, "X", Some("Code:KeyC")));
        // Legacy logical binding: matches the typed key, case-insensitively.
        assert!(binding_matches("x", "X", Some("Code:KeyC")));
        assert!(is_reserved_key_name(&binding_label("Code:KeyT")));
    }

    #[test]
    fn ctrl_function_keys_address_slots_5_to_8() {
        let ctrl = ModifiersState::CONTROL;
        assert_eq!(
            system_action_for(&Key::Named(NamedKey::F2), Pressed, ctrl),
            Some(SystemAction::SaveState(6))
        );
        assert_eq!(
            system_action_for(&Key::Named(NamedKey::F8), Pressed, ctrl),
            Some(SystemAction::LoadState(8))
        );
    }

    #[test]
    fn f12_is_a_reserved_screenshot_key() {
        let m = ModifiersState::empty();
        assert_eq!(
            system_action_for(&Key::Named(NamedKey::F12), Pressed, m),
            Some(SystemAction::Screenshot)
        );
        assert!(is_reserved_key_name("F12"));
    }

    #[test]
    fn backspace_maps_to_rewind_hold() {
        let m = ModifiersState::empty();
        assert_eq!(
            system_action_for(&Key::Named(NamedKey::Backspace), Pressed, m),
            Some(SystemAction::RewindHold(true))
        );
        assert_eq!(
            system_action_for(&Key::Named(NamedKey::Backspace), Released, m),
            Some(SystemAction::RewindHold(false))
        );
    }

    #[test]
    fn backslash_maps_to_rewind_step_on_press_only() {
        let m = ModifiersState::empty();
        assert_eq!(
            system_action_for(&Key::Character("\\"), Pressed, m),
            Some(SystemAction::RewindStep)
        );
        assert_eq!(system_action_for(&Key::Character("\\"), Released, m), None);
    }

    #[test]
    fn rewind_keys_are_reserved() {
        assert!(is_reserved_key_name("Backspace"));
        assert!(is_reserved_key_name("\\"));
    }
}
