use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum TurboSetting { Quarter, Half, Double, Triple, Quadruple, Octuple, Hexadecuple, Uncapped }

impl Default for TurboSetting { fn default() -> Self { TurboSetting::Double } }

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
        a: "Z".into(), b: "X".into(), start: "Enter".into(), select: "Space".into(),
        up: "ArrowUp".into(), down: "ArrowDown".into(), left: "ArrowLeft".into(), right: "ArrowRight".into() } }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Config { pub keybindings: KeyBindings, pub scale: u32, #[serde(default)] pub turbo: TurboSetting }

impl Default for Config { fn default() -> Self { Self { keybindings: KeyBindings::default(), scale: 3, turbo: TurboSetting::default() } } }

impl Config {
    pub fn load(path: &PathBuf) -> Self {
        if let Ok(data) = fs::read_to_string(path) { if let Ok(cfg) = serde_json::from_str::<Config>(&data) { return cfg; } }
        Config::default()
    }
    pub fn save(&self, path: &PathBuf) { if let Ok(data) = serde_json::to_string_pretty(self) { let _ = fs::write(path, data); } }
}

pub fn config_path() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("config.json"))).unwrap_or_else(|| PathBuf::from("config.json"))
}

// Legacy helper used by GUI for translating a winit logical key string to keypad key based on bindings
// pub fn map_winit_key(bindings: &KeyBindings, logical: &winit::keyboard::Key<&str>) -> Option<rust_gbe::KeypadKey> {
//     use winit::keyboard::{Key, NamedKey};
//     match logical {
//         Key::Character(c) => {
//             let upc = c.to_uppercase();
//             if upc == bindings.a { Some(rust_gbe::KeypadKey::A) }
//             else if upc == bindings.b { Some(rust_gbe::KeypadKey::B) }
//             else if upc == bindings.start { Some(rust_gbe::KeypadKey::Start) }
//             else if upc == bindings.select { Some(rust_gbe::KeypadKey::Select) }
//             else { None }
//         }
//         Key::Named(named) => match named {
//             NamedKey::ArrowUp if bindings.up == "ArrowUp" => Some(rust_gbe::KeypadKey::Up),
//             NamedKey::ArrowDown if bindings.down == "ArrowDown" => Some(rust_gbe::KeypadKey::Down),
//             NamedKey::ArrowLeft if bindings.left == "ArrowLeft" => Some(rust_gbe::KeypadKey::Left),
//             NamedKey::ArrowRight if bindings.right == "ArrowRight" => Some(rust_gbe::KeypadKey::Right),
//             NamedKey::Space if bindings.select == "Space" => Some(rust_gbe::KeypadKey::Select),
//             NamedKey::Enter if bindings.start == "Enter" => Some(rust_gbe::KeypadKey::Start),
//             _ => None,
//         },
//         _ => None,
//     }
// }

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

