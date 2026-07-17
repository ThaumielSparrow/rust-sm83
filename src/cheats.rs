//! GameShark cheat model and code parsing. Pure logic — the GUI owns the
//! editor window (gui.rs) and the emulator thread applies pokes per frame.
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One cheat as persisted in config.json (raw code kept for display/editing).
#[derive(Serialize, Deserialize, Clone)]
pub struct Cheat {
    pub code: String,
    pub label: String,
    pub enabled: bool,
}

/// A parsed, applicable RAM poke.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PokeOp {
    /// CGB WRAM bank for 9x codes; None for plain (type 00/01) writes.
    pub bank: Option<u8>,
    pub addr: u16,
    pub value: u8,
}

/// Parse a GameShark code `XXYYZZAA`: XX = type (00/01 plain 8-bit write,
/// 90-97 CGB-WRAM-banked write), YY = value, ZZ = address low byte,
/// AA = address high byte. Whitespace and case are ignored.
pub fn parse_code(input: &str) -> Result<PokeOp, &'static str> {
    let code: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() != 8 {
        return Err("Code must be 8 hex digits");
    }
    let bytes = match u32::from_str_radix(&code, 16) {
        Ok(v) => v.to_be_bytes(),
        Err(_) => return Err("Code must be 8 hex digits"),
    };
    let (ty, value, lo, hi) = (bytes[0], bytes[1], bytes[2], bytes[3]);
    let addr = u16::from_le_bytes([lo, hi]);
    match ty {
        0x00 | 0x01 => Ok(PokeOp { bank: None, addr, value }),
        0x90..=0x97 => Ok(PokeOp { bank: Some(ty & 0x07), addr, value }),
        _ => Err("Unsupported code type (only 01 and 90-97 RAM writes)"),
    }
}

/// Canonical uppercase, whitespace-free form for storage and display.
pub fn normalize_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_uppercase()
}

/// Poke ops for the enabled cheats, skipping unparseable entries (config.json
/// can be hand-edited).
pub fn enabled_pokes(cheats: &[Cheat]) -> Vec<PokeOp> {
    cheats
        .iter()
        .filter(|c| c.enabled)
        .filter_map(|c| parse_code(&c.code).ok())
        .collect()
}

/// Config-storage key for a game's cheats: cartridge header title, falling
/// back to the ROM file name for blank-title ROMs.
pub fn cheat_key(romname: &str, rom_path: &Path) -> String {
    if romname.trim().is_empty() {
        rom_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        romname.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_write() {
        assert_eq!(
            parse_code("01FF56D3"),
            Ok(PokeOp { bank: None, addr: 0xD356, value: 0xFF })
        );
    }

    #[test]
    fn parses_type_00_as_plain_write() {
        assert_eq!(
            parse_code("00FF56D3"),
            Ok(PokeOp { bank: None, addr: 0xD356, value: 0xFF })
        );
    }

    #[test]
    fn parses_banked_write_lowercase_with_spaces() {
        assert_eq!(
            parse_code(" 93 42 cd d2 "),
            Ok(PokeOp { bank: Some(3), addr: 0xD2CD, value: 0x42 })
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_code("0101").is_err(), "too short");
        assert!(parse_code("01FF56D3AA").is_err(), "too long");
        assert!(parse_code("ZZFF56D3").is_err(), "not hex");
        assert!(parse_code("88FF56D3").is_err(), "unsupported type");
    }

    #[test]
    fn normalize_uppercases_and_strips_whitespace() {
        assert_eq!(normalize_code(" 93 42 cd d2 "), "9342CDD2");
    }

    #[test]
    fn enabled_pokes_skips_disabled_and_invalid() {
        let cheats = vec![
            Cheat { code: "01FF56D3".into(), label: "a".into(), enabled: true },
            Cheat { code: "01AA55C0".into(), label: "b".into(), enabled: false },
            Cheat { code: "garbage!".into(), label: "c".into(), enabled: true },
        ];
        assert_eq!(
            enabled_pokes(&cheats),
            vec![PokeOp { bank: None, addr: 0xD356, value: 0xFF }]
        );
    }

    #[test]
    fn cheat_key_prefers_title_falls_back_to_file_name() {
        let p = Path::new("C:/roms/zelda.gbc");
        assert_eq!(cheat_key("ZELDA", p), "ZELDA");
        assert_eq!(cheat_key("   ", p), "zelda.gbc");
    }
}
