//! Pure gamepad logic: name maps, hotkey actions, binding resolution,
//! and the left-stick → d-pad hysteresis mux. No gilrs event-loop code here;
//! the GUI drains gilrs and calls into this module so everything is testable.
use gilrs::Button;
use rust_gbe::KeypadKey;

/// Emulator hotkeys bindable to controller buttons. Mirrors
/// `crate::input::SystemAction`, but without payloads baked into key chords —
/// these are identities used as stable config-file names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HotkeyAction {
    TurboHold,
    TurboToggle,
    Pause,
    SaveState(u8), // slots 1-4
    LoadState(u8), // slots 1-4
    Fullscreen,
    Mute,
    FpsOverlay,
    Interpolation,
    Reset,
}

impl HotkeyAction {
    pub fn all() -> [HotkeyAction; 16] {
        use HotkeyAction::*;
        [
            TurboHold, TurboToggle, Pause,
            SaveState(1), SaveState(2), SaveState(3), SaveState(4),
            LoadState(1), LoadState(2), LoadState(3), LoadState(4),
            Fullscreen, Mute, FpsOverlay, Interpolation, Reset,
        ]
    }

    /// Stable name used as the key in config.json. Never rename these.
    pub fn name(self) -> &'static str {
        use HotkeyAction::*;
        match self {
            TurboHold => "turbo_hold",
            TurboToggle => "turbo_toggle",
            Pause => "pause",
            SaveState(1) => "save_state_1",
            SaveState(2) => "save_state_2",
            SaveState(3) => "save_state_3",
            SaveState(_) => "save_state_4",
            LoadState(1) => "load_state_1",
            LoadState(2) => "load_state_2",
            LoadState(3) => "load_state_3",
            LoadState(_) => "load_state_4",
            Fullscreen => "fullscreen",
            Mute => "mute",
            FpsOverlay => "fps_overlay",
            Interpolation => "interpolation",
            Reset => "reset",
        }
    }

    pub fn from_name(s: &str) -> Option<HotkeyAction> {
        HotkeyAction::all().into_iter().find(|a| a.name() == s)
    }

    pub fn label(self) -> String {
        use HotkeyAction::*;
        match self {
            TurboHold => "Turbo (hold)".into(),
            TurboToggle => "Turbo (toggle)".into(),
            Pause => "Pause".into(),
            SaveState(s) => format!("Save State {}", s),
            LoadState(s) => format!("Load State {}", s),
            Fullscreen => "Toggle Fullscreen".into(),
            Mute => "Toggle Mute".into(),
            FpsOverlay => "FPS Overlay".into(),
            Interpolation => "Linear Interpolation".into(),
            Reset => "Reset Game".into(),
        }
    }

    /// Map a button press/release of this hotkey to a SystemAction.
    /// Only TurboHold cares about release; everything else fires on press.
    pub fn to_system_action(self, pressed: bool) -> Option<crate::input::SystemAction> {
        use crate::input::SystemAction as SA;
        match (self, pressed) {
            (HotkeyAction::TurboHold, p) => Some(SA::TurboHold(p)),
            (_, false) => None,
            (HotkeyAction::TurboToggle, true) => Some(SA::TurboToggle),
            (HotkeyAction::Pause, true) => Some(SA::TogglePause),
            (HotkeyAction::SaveState(s), true) => Some(SA::SaveState(s)),
            (HotkeyAction::LoadState(s), true) => Some(SA::LoadState(s)),
            (HotkeyAction::Fullscreen, true) => Some(SA::ToggleFullscreen),
            (HotkeyAction::Mute, true) => Some(SA::ToggleMute),
            (HotkeyAction::FpsOverlay, true) => Some(SA::ToggleFpsOverlay),
            (HotkeyAction::Interpolation, true) => Some(SA::ToggleInterpolation),
            (HotkeyAction::Reset, true) => Some(SA::Reset),
        }
    }
}

/// Stable string for a gilrs button, used as the value in config.json.
/// Explicit list (not Debug formatting) so config stability doesn't depend on
/// gilrs internals. Unknown/unnamed buttons are not bindable.
pub fn button_name(b: Button) -> Option<&'static str> {
    Some(match b {
        Button::South => "South",
        Button::East => "East",
        Button::North => "North",
        Button::West => "West",
        Button::C => "C",
        Button::Z => "Z",
        Button::LeftTrigger => "LeftTrigger",
        Button::LeftTrigger2 => "LeftTrigger2",
        Button::RightTrigger => "RightTrigger",
        Button::RightTrigger2 => "RightTrigger2",
        Button::Select => "Select",
        Button::Start => "Start",
        Button::Mode => "Mode",
        Button::LeftThumb => "LeftThumb",
        Button::RightThumb => "RightThumb",
        Button::DPadUp => "DPadUp",
        Button::DPadDown => "DPadDown",
        Button::DPadLeft => "DPadLeft",
        Button::DPadRight => "DPadRight",
        _ => return None,
    })
}

pub fn button_from_name(s: &str) -> Option<Button> {
    Some(match s {
        "South" => Button::South,
        "East" => Button::East,
        "North" => Button::North,
        "West" => Button::West,
        "C" => Button::C,
        "Z" => Button::Z,
        "LeftTrigger" => Button::LeftTrigger,
        "LeftTrigger2" => Button::LeftTrigger2,
        "RightTrigger" => Button::RightTrigger,
        "RightTrigger2" => Button::RightTrigger2,
        "Select" => Button::Select,
        "Start" => Button::Start,
        "Mode" => Button::Mode,
        "LeftThumb" => Button::LeftThumb,
        "RightThumb" => Button::RightThumb,
        "DPadUp" => Button::DPadUp,
        "DPadDown" => Button::DPadDown,
        "DPadLeft" => Button::DPadLeft,
        "DPadRight" => Button::DPadRight,
        _ => return None,
    })
}

/// Stable config-file name for a GB button.
pub fn keypad_key_name(k: KeypadKey) -> &'static str {
    match k {
        KeypadKey::A => "a",
        KeypadKey::B => "b",
        KeypadKey::Start => "start",
        KeypadKey::Select => "select",
        KeypadKey::Up => "up",
        KeypadKey::Down => "down",
        KeypadKey::Left => "left",
        KeypadKey::Right => "right",
    }
}

pub fn keypad_key_from_name(s: &str) -> Option<KeypadKey> {
    Some(match s {
        "a" => KeypadKey::A,
        "b" => KeypadKey::B,
        "start" => KeypadKey::Start,
        "select" => KeypadKey::Select,
        "up" => KeypadKey::Up,
        "down" => KeypadKey::Down,
        "left" => KeypadKey::Left,
        "right" => KeypadKey::Right,
        _ => return None,
    })
}

/// Left-stick press/release thresholds with hysteresis (spec: press past 0.5,
/// release below 0.35) so values jittering near the threshold don't chatter.
pub const STICK_PRESS: f32 = 0.5;
pub const STICK_RELEASE: f32 = 0.35;

const DIR_UP: usize = 0;
const DIR_DOWN: usize = 1;
const DIR_LEFT: usize = 2;
const DIR_RIGHT: usize = 3;
const DIRS: [KeypadKey; 4] = [KeypadKey::Up, KeypadKey::Down, KeypadKey::Left, KeypadKey::Right];

fn dir_index(k: KeypadKey) -> Option<usize> {
    match k {
        KeypadKey::Up => Some(DIR_UP),
        KeypadKey::Down => Some(DIR_DOWN),
        KeypadKey::Left => Some(DIR_LEFT),
        KeypadKey::Right => Some(DIR_RIGHT),
        _ => None,
    }
}

/// `magnitude` is the axis deflection *toward* the direction (always >= 0 when
/// the stick points that way).
fn hysteresis(held: bool, magnitude: f32) -> bool {
    if magnitude > STICK_PRESS {
        true
    } else if magnitude < STICK_RELEASE {
        false
    } else {
        held
    }
}

/// Tracks stick-derived and button-derived direction state separately and
/// emits a press/release transition only when the OR of the two changes.
/// Non-direction GB buttons pass through `set_button` untouched.
#[derive(Default)]
pub struct DirectionMux {
    stick: [bool; 4],
    button: [bool; 4],
}

impl DirectionMux {
    fn combined(&self, i: usize) -> bool {
        self.stick[i] || self.button[i]
    }

    fn update_axis_pair(
        &mut self,
        neg_dir: usize,
        pos_dir: usize,
        v: f32,
        out: &mut Vec<(KeypadKey, bool)>,
    ) {
        let before = [self.combined(neg_dir), self.combined(pos_dir)];
        self.stick[neg_dir] = hysteresis(self.stick[neg_dir], -v);
        self.stick[pos_dir] = hysteresis(self.stick[pos_dir], v);
        let after = [self.combined(neg_dir), self.combined(pos_dir)];
        if before[0] != after[0] {
            out.push((DIRS[neg_dir], after[0]));
        }
        if before[1] != after[1] {
            out.push((DIRS[pos_dir], after[1]));
        }
    }

    pub fn set_stick_x(&mut self, v: f32, out: &mut Vec<(KeypadKey, bool)>) {
        self.update_axis_pair(DIR_LEFT, DIR_RIGHT, v, out);
    }

    /// gilrs convention: positive LeftStickY is up.
    pub fn set_stick_y(&mut self, v: f32, out: &mut Vec<(KeypadKey, bool)>) {
        self.update_axis_pair(DIR_DOWN, DIR_UP, v, out);
    }

    pub fn set_button(&mut self, k: KeypadKey, pressed: bool, out: &mut Vec<(KeypadKey, bool)>) {
        let Some(i) = dir_index(k) else {
            out.push((k, pressed));
            return;
        };
        let before = self.combined(i);
        self.button[i] = pressed;
        let after = self.combined(i);
        if before != after {
            out.push((DIRS[i], after));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_BUTTONS: [Button; 19] = [
        Button::South, Button::East, Button::North, Button::West,
        Button::C, Button::Z,
        Button::LeftTrigger, Button::LeftTrigger2,
        Button::RightTrigger, Button::RightTrigger2,
        Button::Select, Button::Start, Button::Mode,
        Button::LeftThumb, Button::RightThumb,
        Button::DPadUp, Button::DPadDown, Button::DPadLeft, Button::DPadRight,
    ];

    #[test]
    fn button_names_round_trip() {
        for b in ALL_BUTTONS {
            let name = button_name(b).expect("named button");
            assert_eq!(button_from_name(name), Some(b), "{}", name);
        }
        assert_eq!(button_from_name("NotAButton"), None);
        assert_eq!(button_name(Button::Unknown), None);
    }

    #[test]
    fn keypad_key_names_round_trip() {
        for k in [
            KeypadKey::A, KeypadKey::B, KeypadKey::Start, KeypadKey::Select,
            KeypadKey::Up, KeypadKey::Down, KeypadKey::Left, KeypadKey::Right,
        ] {
            assert_eq!(keypad_key_from_name(keypad_key_name(k)), Some(k));
        }
        assert_eq!(keypad_key_from_name("x"), None);
    }

    #[test]
    fn hotkey_names_round_trip_and_are_unique() {
        let all = HotkeyAction::all();
        for a in all {
            assert_eq!(HotkeyAction::from_name(a.name()), Some(a));
        }
        let mut names: Vec<_> = all.iter().map(|a| a.name()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all.len(), "duplicate hotkey names");
    }

    #[test]
    fn turbo_hold_maps_press_and_release() {
        use crate::input::SystemAction;
        assert!(matches!(
            HotkeyAction::TurboHold.to_system_action(true),
            Some(SystemAction::TurboHold(true))
        ));
        assert!(matches!(
            HotkeyAction::TurboHold.to_system_action(false),
            Some(SystemAction::TurboHold(false))
        ));
        // Non-hold hotkeys ignore release.
        assert!(HotkeyAction::Pause.to_system_action(false).is_none());
        assert!(matches!(
            HotkeyAction::SaveState(3).to_system_action(true),
            Some(SystemAction::SaveState(3))
        ));
    }

    #[test]
    fn stick_hysteresis_press_hold_release() {
        let mut mux = DirectionMux::default();
        let mut out = Vec::new();

        mux.set_stick_x(-0.6, &mut out); // past press threshold
        assert_eq!(out, vec![(KeypadKey::Left, true)]);

        out.clear();
        mux.set_stick_x(-0.4, &mut out); // in hysteresis band: still held
        assert_eq!(out, vec![]);

        out.clear();
        mux.set_stick_x(-0.3, &mut out); // below release threshold
        assert_eq!(out, vec![(KeypadKey::Left, false)]);

        out.clear();
        mux.set_stick_x(-0.45, &mut out); // band entered from below: NOT pressed
        assert_eq!(out, vec![]);
    }

    #[test]
    fn stick_y_positive_is_up() {
        let mut mux = DirectionMux::default();
        let mut out = Vec::new();
        mux.set_stick_y(0.8, &mut out);
        assert_eq!(out, vec![(KeypadKey::Up, true)]);
        out.clear();
        mux.set_stick_y(-0.8, &mut out);
        // Up releases and Down presses.
        assert!(out.contains(&(KeypadKey::Up, false)));
        assert!(out.contains(&(KeypadKey::Down, true)));
    }

    #[test]
    fn stick_and_dpad_or_together() {
        let mut mux = DirectionMux::default();
        let mut out = Vec::new();

        mux.set_stick_x(0.9, &mut out); // stick holds Right
        assert_eq!(out, vec![(KeypadKey::Right, true)]);

        out.clear();
        mux.set_button(KeypadKey::Right, true, &mut out); // d-pad also Right
        assert_eq!(out, vec![], "already held: no duplicate press");

        out.clear();
        mux.set_button(KeypadKey::Right, false, &mut out); // d-pad released
        assert_eq!(out, vec![], "stick still holds it: no release");

        out.clear();
        mux.set_stick_x(0.0, &mut out); // stick released too
        assert_eq!(out, vec![(KeypadKey::Right, false)]);
    }

    #[test]
    fn non_direction_buttons_pass_through() {
        let mut mux = DirectionMux::default();
        let mut out = Vec::new();
        mux.set_button(KeypadKey::A, true, &mut out);
        mux.set_button(KeypadKey::A, false, &mut out);
        assert_eq!(out, vec![(KeypadKey::A, true), (KeypadKey::A, false)]);
    }
}
