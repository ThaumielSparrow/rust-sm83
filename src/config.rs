use serde::{Deserialize, Serialize};
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

impl Default for KeyBindings {
    fn default() -> Self { Self {
        a: "X".into(), b: "Z".into(), start: "Enter".into(), select: "Space".into(),
        up: "ArrowUp".into(), down: "ArrowDown".into(), left: "ArrowLeft".into(), right: "ArrowRight".into() } }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
    pub keybindings: KeyBindings,
    pub scale: u32,
    #[serde(default)] pub turbo: TurboSetting,
    #[serde(default="default_volume")] pub volume: u8, // 0-100 user slider value (perceptual)
    #[serde(default)] pub recent_roms: Vec<String>,
    #[serde(default)] pub fps_overlay: bool,
    #[serde(default)] pub fullscreen: bool,
    #[serde(default)] pub dmg_palette_preset: DmgPalettePreset,
    #[serde(default="default_custom_palette")] pub dmg_palette_custom: [[u8; 3]; 4],
}

fn default_volume() -> u8 { 100 }

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
            dmg_palette_preset: DmgPalettePreset::default(),
            dmg_palette_custom: default_custom_palette(),
        }
    }
}

impl Config {
    pub fn load(path: &PathBuf) -> Self {
        if let Ok(data) = fs::read_to_string(path) { if let Ok(cfg) = serde_json::from_str::<Config>(&data) { return cfg; } }
        Config::default()
    }
    pub fn save(&self, path: &PathBuf) { if let Ok(data) = serde_json::to_string_pretty(self) { let _ = fs::write(path, data); } }

    /// Insert `p` at the front of recent_roms, deduplicating and truncating to 8.
    pub fn push_recent(&mut self, p: &Path) {
        let s = p.to_string_lossy().into_owned();
        self.recent_roms.retain(|x| x != &s);
        self.recent_roms.insert(0, s);
        self.recent_roms.truncate(8);
    }
}

/// Load config, apply mutation, save. Used by GUI when a single setting changes —
/// avoids each call site needing to know every Config field.
pub fn update_config<F: FnOnce(&mut Config)>(f: F) {
    let mut cfg = Config::load(&config_path());
    f(&mut cfg);
    cfg.save(&config_path());
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

