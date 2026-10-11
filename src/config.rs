use serde::{Deserialize, Serialize};
use rust_gbe::KeypadKey;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum TurboSetting { Quarter, Half, Double, Triple, Quadruple, Octuple, Hexadecuple, Uncapped }

impl Default for TurboSetting { fn default() -> Self { TurboSetting::Double } }

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum DmgPalettePreset { Green, Pocket, Light, Custom }

impl Default for DmgPalettePreset { fn default() -> Self { DmgPalettePreset::Green } }

impl DmgPalettePreset {
    pub fn all() -> &'static [DmgPalettePreset] {
        &[DmgPalettePreset::Green, DmgPalettePreset::Pocket, DmgPalettePreset::Light, DmgPalettePreset::Custom]
    }
    pub fn label(&self) -> &'static str {
        match self {
            DmgPalettePreset::Green => "Green (DMG)",
            DmgPalettePreset::Pocket => "Pocket (Grey)",
            DmgPalettePreset::Light => "Light",
            DmgPalettePreset::Custom => "Custom",
        }
    }
}

fn default_custom_palette() -> [[u8; 3]; 4] {
    [[255, 255, 255], [180, 180, 180], [110, 110, 110], [40, 40, 40]]
}

impl TurboSetting {
    pub fn all() -> &'static [TurboSetting] { &[
        TurboSetting::Quarter,
        TurboSetting::Half,
        TurboSetting::Double,
        TurboSetting::Triple,
        TurboSetting::Quadruple,
        TurboSetting::Octuple,
        TurboSetting::Hexadecuple,
        TurboSetting::Uncapped,
    ] }
    pub fn label(&self) -> &'static str { match self { TurboSetting::Quarter=>"0.25x", TurboSetting::Half=>"0.5x", TurboSetting::Double=>"2x", TurboSetting::Triple=>"3x", TurboSetting::Quadruple=>"4x", TurboSetting::Octuple=>"8x", TurboSetting::Hexadecuple=>"16x", TurboSetting::Uncapped=>"Uncapped" } }
    pub fn multiplier(&self) -> Option<f32> { match self { TurboSetting::Quarter=>Some(0.25), TurboSetting::Half=>Some(0.5), TurboSetting::Double=>Some(2.0), TurboSetting::Triple=>Some(3.0), TurboSetting::Quadruple=>Some(4.0), TurboSetting::Octuple=>Some(8.0), TurboSetting::Hexadecuple=>Some(16.0), TurboSetting::Uncapped=>None } }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct KeyBindings {
    pub a: String,
    pub b: String,
    pub start: String,
    pub select: String,
    pub up: String,
    pub down: String,
    pub left: String,
    pub right: String,
}

impl KeyBindings {
    fn slot_mut(&mut self, k: KeypadKey) -> &mut String {
        match k {
            KeypadKey::A => &mut self.a,
            KeypadKey::B => &mut self.b,
            KeypadKey::Start => &mut self.start,
            KeypadKey::Select => &mut self.select,
            KeypadKey::Up => &mut self.up,
            KeypadKey::Down => &mut self.down,
            KeypadKey::Left => &mut self.left,
            KeypadKey::Right => &mut self.right,
        }
    }

    /// Bind `value` (a logical key name or a `Code:` physical binding) to `k`.
    /// Reserved system keys are refused (returns false).
    /// If another Game Boy button already uses `value`, the two swap so no
    /// key ends up driving two buttons.
    pub fn assign(&mut self, k: KeypadKey, value: String) -> bool {
        if crate::input::is_reserved_key_name(&crate::input::binding_label(&value)) {
            return false;
        }
        let old = self.slot_mut(k).clone();
        for other in [
            KeypadKey::A,
            KeypadKey::B,
            KeypadKey::Start,
            KeypadKey::Select,
            KeypadKey::Up,
            KeypadKey::Down,
            KeypadKey::Left,
            KeypadKey::Right,
        ] {
            if other != k && *self.slot_mut(other) == value {
                *self.slot_mut(other) = old.clone();
            }
        }
        *self.slot_mut(k) = value;
        true
    }
}

impl Default for KeyBindings {
    fn default() -> Self { Self {
        // Physical positions, so the defaults sit in the same place on any layout.
        a: "Code:KeyX".into(), b: "Code:KeyZ".into(), start: "Code:Enter".into(), select: "Code:Space".into(),
        up: "Code:ArrowUp".into(), down: "Code:ArrowDown".into(), left: "Code:ArrowLeft".into(), right: "Code:ArrowRight".into() } }
}

/// Controller bindings, kept separate from keyboard bindings. Both maps store
/// stable string names (see src/gamepad.rs) so the config file survives enum
/// changes. `buttons`: GB button name -> gilrs button name. `hotkeys`:
/// hotkey action name -> gilrs button name.
#[derive(Serialize, Deserialize, Clone)]
pub struct GamepadBindings {
    #[serde(default)]
    pub buttons: HashMap<String, String>,
    #[serde(default)]
    pub hotkeys: HashMap<String, String>,
}

impl Default for GamepadBindings {
    fn default() -> Self {
        // Label-matching layout: the button physically labeled A on an
        // Xbox-style pad (gilrs South) maps to GB A, East (B) to GB B.
        let buttons = [
            ("a", "South"),
            ("b", "East"),
            ("start", "Start"),
            ("select", "Select"),
            ("up", "DPadUp"),
            ("down", "DPadDown"),
            ("left", "DPadLeft"),
            ("right", "DPadRight"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        GamepadBindings { buttons, hotkeys: HashMap::new() }
    }
}

/// Missing fields fall back to `Config::default()` (or their field-level
/// default) so an older or hand-trimmed config.json still loads.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub keybindings: KeyBindings,
    pub scale: u32,
    #[serde(default)] pub turbo: TurboSetting,
    #[serde(default="default_volume")] pub volume: u8, // 0-100 user slider value (perceptual)
    #[serde(default)] pub recent_roms: Vec<String>,
    #[serde(default)] pub fps_overlay: bool,
    #[serde(default)] pub fullscreen: bool,
    #[serde(default)] pub integer_scaling: bool,
    #[serde(default = "default_true")] pub auto_resume: bool,
    /// Per-game cheat lists, keyed by cartridge header title (file name for
    /// blank-title ROMs — see cheats::cheat_key).
    #[serde(default)] pub cheats: HashMap<String, Vec<crate::cheats::Cheat>>,
    #[serde(default)] pub dmg_palette_preset: DmgPalettePreset,
    #[serde(default="default_custom_palette")] pub dmg_palette_custom: [[u8; 3]; 4],
    #[serde(default)] pub gamepad: GamepadBindings,
    /// Main window's last outer position (physical px), restored at startup.
    #[serde(default)] pub window_pos: Option<(i32, i32)>,
    /// Pause emulation while the window is in the background.
    #[serde(default)] pub pause_on_focus_loss: bool,
    #[serde(default)] pub screen_filter: ScreenFilter,
    /// Darken the edges of each Game Boy pixel, like the original LCD.
    #[serde(default)] pub lcd_grid: bool,
    /// Apply the CGB LCD color correction (off = raw 15-bit colors).
    #[serde(default = "default_true")] pub cgb_color_correction: bool,
}

/// How the 160x144 frame is scaled up to the window.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ScreenFilter {
    /// Hard square pixels; uneven pixel widths at non-integer scales.
    #[default]
    Nearest,
    /// Smooth, but blurry.
    Bilinear,
    /// Square pixels with only their edges blended: crisp at any scale.
    SharpBilinear,
}

impl ScreenFilter {
    pub fn all() -> &'static [ScreenFilter] {
        &[ScreenFilter::Nearest, ScreenFilter::Bilinear, ScreenFilter::SharpBilinear]
    }
    pub fn label(&self) -> &'static str {
        match self {
            ScreenFilter::Nearest => "Nearest",
            ScreenFilter::Bilinear => "Bilinear",
            ScreenFilter::SharpBilinear => "Sharp bilinear",
        }
    }
    /// The next filter, for the cycle hotkey.
    pub fn next(&self) -> ScreenFilter {
        match self {
            ScreenFilter::Nearest => ScreenFilter::Bilinear,
            ScreenFilter::Bilinear => ScreenFilter::SharpBilinear,
            ScreenFilter::SharpBilinear => ScreenFilter::Nearest,
        }
    }
}

static LOAD_WARNING: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The message from the last `Config::load` that had to discard a corrupt
/// file, if any. Cleared on read.
pub fn take_load_warning() -> Option<String> {
    LOAD_WARNING.lock().ok().and_then(|mut w| w.take())
}

fn default_volume() -> u8 { 100 }

fn default_true() -> bool { true }

impl Default for Config {
    fn default() -> Self {
        Self {
            keybindings: KeyBindings::default(),
            scale: 3,
            turbo: TurboSetting::default(),
            volume: default_volume(),
            recent_roms: Vec::new(),
            fps_overlay: false,
            fullscreen: false,
            integer_scaling: false,
            auto_resume: true,
            cheats: HashMap::new(),
            dmg_palette_preset: DmgPalettePreset::default(),
            dmg_palette_custom: default_custom_palette(),
            gamepad: GamepadBindings::default(),
            window_pos: None,
            pause_on_focus_loss: false,
            screen_filter: ScreenFilter::default(),
            lcd_grid: false,
            cgb_color_correction: true,
        }
    }
}

impl Config {
    /// Load the config, or defaults if there is none. A file that exists but
    /// doesn't parse is moved aside (never silently overwritten by the next
    /// save) and a warning is queued for `take_load_warning`.
    pub fn load(path: &PathBuf) -> Self {
        let Ok(data) = fs::read_to_string(path) else {
            return Config::default();
        };
        match serde_json::from_str::<Config>(&data) {
            Ok(cfg) => cfg,
            Err(e) => {
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let backup = path.with_extension(format!("json.bad-{stamp}"));
                let msg = match fs::rename(path, &backup) {
                    Ok(()) => format!(
                        "config.json was unreadable ({}); using defaults. The old file was kept as {}",
                        e,
                        backup.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    Err(_) => format!("config.json was unreadable ({e}); using defaults"),
                };
                crate::log::warn(&msg);
                if let Ok(mut w) = LOAD_WARNING.lock() {
                    *w = Some(msg);
                }
                Config::default()
            }
        }
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| format!("config serialize: {}", e))?;
        let tmp = path.with_extension("json.tmp");
        {
            use std::io::Write;
            let mut f = fs::File::create(&tmp).map_err(|e| format!("config create tmp: {}", e))?;
            f.write_all(data.as_bytes())
                .map_err(|e| format!("config write tmp: {}", e))?;
            // fsync before rename so a crash can't leave a renamed-but-empty config.
            f.sync_all().map_err(|e| format!("config sync tmp: {}", e))?;
        }
        fs::rename(&tmp, path).map_err(|e| format!("config rename: {}", e))?;
        Ok(())
    }

    /// Insert `p` at the front of recent_roms, deduplicating and truncating to 8.
    pub fn push_recent(&mut self, p: &Path) {
        push_recent(&mut self.recent_roms, p);
    }
}

/// Insert `p` at the front of a recent-ROMs list, deduplicating and truncating to 8.
pub fn push_recent(list: &mut Vec<String>, p: &Path) {
    let s = p.to_string_lossy().into_owned();
    list.retain(|x| x != &s);
    list.insert(0, s);
    list.truncate(8);
}

/// Load config, apply mutation, save. Used by GUI when a single setting changes —
/// avoids each call site needing to know every Config field.
pub fn update_config<F: FnOnce(&mut Config)>(f: F) {
    // Apply on top of any not-yet-flushed debounced edits so none are lost.
    let pending = PENDING.lock().ok().and_then(|mut p| p.take());
    let mut cfg = pending.map(|(c, _)| c).unwrap_or_else(|| Config::load(&config_path()));
    f(&mut cfg);
    if let Err(e) = cfg.save(&config_path()) {
        crate::log::warn(format!("config save failed: {}", e));
    }
}

/// Debounced edits: the config with changes applied, and when it last changed.
static PENDING: std::sync::Mutex<Option<(Config, std::time::Instant)>> = std::sync::Mutex::new(None);

/// How long a debounced edit waits for the next one before it is written.
const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(500);

/// Like `update_config`, but for rapid-fire edits (slider drags, color
/// pickers): changes are held in memory and written once they stop for
/// `DEBOUNCE`, by `flush_pending_config`.
pub fn update_config_debounced<F: FnOnce(&mut Config)>(f: F) {
    if let Ok(mut pending) = PENDING.lock() {
        let entry = pending
            .get_or_insert_with(|| (Config::load(&config_path()), std::time::Instant::now()));
        f(&mut entry.0);
        entry.1 = std::time::Instant::now();
    }
}

/// Write debounced edits that have settled, or all of them when `force`
/// (on exit). Called regularly from the GUI loop.
pub fn flush_pending_config(force: bool) {
    let ready = PENDING.lock().ok().and_then(|mut p| {
        if p.as_ref().is_some_and(|(_, at)| force || at.elapsed() >= DEBOUNCE) {
            p.take()
        } else {
            None
        }
    });
    if let Some((cfg, _)) = ready
        && let Err(e) = cfg.save(&config_path())
    {
        crate::log::warn(format!("config save failed: {}", e));
    }
}

pub fn config_path() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("config.json"))).unwrap_or_else(|| PathBuf::from("config.json"))
}

// Provide display text for a keypad key's binding value
pub fn binding_value(bindings: &KeyBindings, key: rust_gbe::KeypadKey) -> String {
    match key {
        rust_gbe::KeypadKey::A => bindings.a.clone(),
        rust_gbe::KeypadKey::B => bindings.b.clone(),
        rust_gbe::KeypadKey::Start => bindings.start.clone(),
        rust_gbe::KeypadKey::Select => bindings.select.clone(),
        rust_gbe::KeypadKey::Up => bindings.up.clone(),
        rust_gbe::KeypadKey::Down => bindings.down.clone(),
        rust_gbe::KeypadKey::Left => bindings.left.clone(),
        rust_gbe::KeypadKey::Right => bindings.right.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_without_gamepad_field_gets_defaults() {
        // Minimal pre-gamepad config.json: only fields without serde defaults.
        let json = r#"{
            "keybindings": {"a":"X","b":"Z","start":"Enter","select":"Space",
                            "up":"ArrowUp","down":"ArrowDown","left":"ArrowLeft","right":"ArrowRight"},
            "scale": 3
        }"#;
        let cfg: Config = serde_json::from_str(json).expect("parse");
        assert_eq!(cfg.gamepad.buttons.get("a").map(String::as_str), Some("South"));
        assert!(cfg.gamepad.hotkeys.is_empty());
    }

    #[test]
    fn cheats_round_trip_and_default_empty() {
        let mut cfg = Config::default();
        assert!(cfg.cheats.is_empty());
        cfg.cheats.insert(
            "ZELDA".into(),
            vec![crate::cheats::Cheat { code: "01FF56D3".into(), label: "hp".into(), enabled: true }],
        );
        let json = serde_json::to_string(&cfg).expect("serialize");
        let back: Config = serde_json::from_str(&json).expect("parse");
        assert_eq!(back.cheats["ZELDA"].len(), 1);
        assert!(back.cheats["ZELDA"][0].enabled);
    }

    #[test]
    fn empty_object_loads_as_defaults() {
        let cfg: Config = serde_json::from_str("{}").expect("parse");
        assert_eq!(cfg.scale, Config::default().scale);
        assert_eq!(cfg.volume, 100);
        assert_eq!(cfg.keybindings.a, "Code:KeyX");
    }

    #[test]
    fn corrupt_config_is_moved_aside_not_lost() {
        let dir = std::env::temp_dir().join(format!("rgbe-cfg-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        fs::write(&path, "{ not json").unwrap();
        let cfg = Config::load(&path);
        assert_eq!(cfg.scale, Config::default().scale);
        assert!(!path.exists(), "corrupt file must be moved, not left to be overwritten");
        let backups: Vec<_> = fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read_to_string(backups[0].path()).unwrap(), "{ not json");
        assert!(take_load_warning().is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn assign_refuses_reserved_and_swaps_duplicates() {
        let mut kb = KeyBindings::default();
        assert!(!kb.assign(KeypadKey::A, "F1".into()), "F1 is a reserved save key");
        assert!(!kb.assign(KeypadKey::A, "Code:KeyT".into()), "physical T is reserved too");
        assert_eq!(kb.a, "Code:KeyX");
        assert!(kb.assign(KeypadKey::A, "Code:KeyZ".into()));
        assert_eq!(kb.a, "Code:KeyZ");
        assert_eq!(kb.b, "Code:KeyX", "B previously held Z and takes A's old key");
    }

    #[test]
    fn gamepad_bindings_round_trip() {
        let mut cfg = Config::default();
        cfg.gamepad.hotkeys.insert("pause".into(), "North".into());
        let json = serde_json::to_string(&cfg).expect("serialize");
        let back: Config = serde_json::from_str(&json).expect("parse");
        assert_eq!(back.gamepad.buttons, cfg.gamepad.buttons);
        assert_eq!(back.gamepad.hotkeys.get("pause").map(String::as_str), Some("North"));
    }
}
