use crate::apu;
use crate::cpu::CPU;
use crate::gbmode::GbMode;
use crate::keypad::KeypadKey;
use crate::mbc::{self, MBC};
use crate::StrResult;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Device {
    cpu: CPU,
    save_state: Option<String>,
}

const SAVE_STATE_MAGIC: &[u8; 8] = b"RGBEST04";
const SAVE_STATE_HEADER_LEN: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveStatePreview {
    pub saved_at_unix_secs: u64,
    pub thumbnail_width: u16,
    pub thumbnail_height: u16,
    pub thumbnail_rgb: Option<Vec<u8>>,
}

struct SaveStateParts<'a> {
    cpu_payload: &'a [u8],
}

struct SaveStateHeader {
    saved_at_unix_secs: u64,
    thumbnail_width: u16,
    thumbnail_height: u16,
    thumbnail_len: usize,
    cpu_payload_len: usize,
}

fn write_atomic(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(match path.extension() {
        Some(ext) => format!("{}.tmp", ext.to_string_lossy()),
        None => "tmp".to_string(),
    });
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        // Flush the temp file's contents to disk before the rename so a crash
        // can't leave a renamed-but-empty file (rename is atomic, write isn't).
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn encode_cpu_state(cpu: &CPU) -> StrResult<Vec<u8>> {
    // Auto-save uses the same on-disk format as slot saves, just without a
    // thumbnail. One format keeps decode_cpu_state unambiguous.
    encode_cpu_state_with_preview(cpu, None).map(|(data, _)| data)
}

fn encode_cpu_state_with_preview(
    cpu: &CPU,
    thumbnail_rgb: Option<&[u8]>,
) -> StrResult<(Vec<u8>, SaveStatePreview)> {
    let payload =
        rkyv::to_bytes::<rkyv::rancor::Error>(cpu).map_err(|_| "Failed to serialize CPU state")?;
    let thumbnail_rgb =
        thumbnail_rgb.filter(|data| data.len() == crate::gpu::SCREEN_W * crate::gpu::SCREEN_H * 3);
    let thumbnail_len = thumbnail_rgb.map_or(0, <[u8]>::len);
    let thumbnail_len_u32 = u32::try_from(thumbnail_len).map_err(|_| "Thumbnail is too large")?;
    let payload_len_u64 = u64::try_from(payload.len()).map_err(|_| "Save state is too large")?;
    let saved_at_unix_secs = current_unix_secs();
    let thumbnail_width = if thumbnail_rgb.is_some() {
        crate::gpu::SCREEN_W as u16
    } else {
        0
    };
    let thumbnail_height = if thumbnail_rgb.is_some() {
        crate::gpu::SCREEN_H as u16
    } else {
        0
    };

    let mut data = Vec::with_capacity(SAVE_STATE_HEADER_LEN + thumbnail_len + payload.len());
    data.extend_from_slice(SAVE_STATE_MAGIC);
    data.extend_from_slice(&saved_at_unix_secs.to_le_bytes());
    data.extend_from_slice(&thumbnail_width.to_le_bytes());
    data.extend_from_slice(&thumbnail_height.to_le_bytes());
    data.extend_from_slice(&thumbnail_len_u32.to_le_bytes());
    data.extend_from_slice(&payload_len_u64.to_le_bytes());
    if let Some(thumbnail_rgb) = thumbnail_rgb {
        data.extend_from_slice(thumbnail_rgb);
    }
    data.extend_from_slice(&payload);

    let preview = SaveStatePreview {
        saved_at_unix_secs,
        thumbnail_width,
        thumbnail_height,
        thumbnail_rgb: thumbnail_rgb.map(<[u8]>::to_vec),
    };

    Ok((data, preview))
}

fn decode_cpu_state(data: &[u8]) -> StrResult<CPU> {
    if !data.starts_with(SAVE_STATE_MAGIC) {
        return Err("Unsupported save state format");
    }
    let payload = parse_save_state(data)
        .ok_or("Failed to parse save state")?
        .cpu_payload;

    rkyv::from_bytes::<CPU, rkyv::rancor::Error>(payload).map_err(|_| "Failed to parse save state")
}

fn parent_dir(path: &str) -> Option<std::path::PathBuf> {
    std::path::Path::new(path)
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(std::path::Path::to_path_buf)
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn read_u16_le(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_u32_le(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_u64_le(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        data.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn parse_save_state(data: &[u8]) -> Option<SaveStateParts<'_>> {
    let header = parse_save_state_header(data, data.len())?;

    let thumbnail_start = SAVE_STATE_HEADER_LEN;
    let thumbnail_end = thumbnail_start.checked_add(header.thumbnail_len)?;
    let cpu_payload_end = thumbnail_end.checked_add(header.cpu_payload_len)?;

    if header.thumbnail_len != 0 {
        data.get(thumbnail_start..thumbnail_end)?;
    }
    let cpu_payload = data.get(thumbnail_end..cpu_payload_end)?;

    Some(SaveStateParts { cpu_payload })
}

fn parse_save_state_header(data: &[u8], total_len: usize) -> Option<SaveStateHeader> {
    if !data.starts_with(SAVE_STATE_MAGIC) || data.len() < SAVE_STATE_HEADER_LEN {
        return None;
    }

    let saved_at_unix_secs = read_u64_le(data, 8)?;
    let thumbnail_width = read_u16_le(data, 16)?;
    let thumbnail_height = read_u16_le(data, 18)?;
    let thumbnail_len = usize::try_from(read_u32_le(data, 20)?).ok()?;
    let cpu_payload_len = usize::try_from(read_u64_le(data, 24)?).ok()?;
    if cpu_payload_len == 0 {
        return None;
    }

    if thumbnail_len == 0 {
        if thumbnail_width != 0 || thumbnail_height != 0 {
            return None;
        }
    } else {
        // Pin thumbnail dimensions to SCREEN_W x SCREEN_H. Any other dimensions are
        // suspicious (truncated file, format drift, hostile file) and rejected.
        if thumbnail_width as usize != crate::gpu::SCREEN_W
            || thumbnail_height as usize != crate::gpu::SCREEN_H
        {
            return None;
        }
        let expected_thumbnail_len = usize::from(thumbnail_width)
            .checked_mul(usize::from(thumbnail_height))?
            .checked_mul(3)?;
        if thumbnail_len != expected_thumbnail_len {
            return None;
        }
    }

    let expected_len = SAVE_STATE_HEADER_LEN
        .checked_add(thumbnail_len)?
        .checked_add(cpu_payload_len)?;
    if total_len != expected_len {
        return None;
    }

    Some(SaveStateHeader {
        saved_at_unix_secs,
        thumbnail_width,
        thumbnail_height,
        thumbnail_len,
        cpu_payload_len,
    })
}

pub fn read_save_state_preview(path: impl AsRef<Path>) -> Option<SaveStatePreview> {
    let path = path.as_ref();
    let mut file = std::fs::File::open(path).ok()?;
    let mut magic = [0; 8];
    file.read_exact(&mut magic).ok()?;

    if &magic != SAVE_STATE_MAGIC {
        return None;
    }

    let total_len = usize::try_from(file.metadata().ok()?.len()).ok()?;
    let mut header_data = [0; SAVE_STATE_HEADER_LEN];
    header_data[..SAVE_STATE_MAGIC.len()].copy_from_slice(&magic);
    file.read_exact(&mut header_data[SAVE_STATE_MAGIC.len()..])
        .ok()?;
    let header = parse_save_state_header(&header_data, total_len)?;
    let thumbnail_rgb = if header.thumbnail_len == 0 {
        None
    } else {
        let mut thumbnail_rgb = vec![0; header.thumbnail_len];
        file.read_exact(&mut thumbnail_rgb).ok()?;
        Some(thumbnail_rgb)
    };

    Some(SaveStatePreview {
        saved_at_unix_secs: header.saved_at_unix_secs,
        thumbnail_width: header.thumbnail_width,
        thumbnail_height: header.thumbnail_height,
        thumbnail_rgb,
    })
}

impl Drop for Device {
    fn drop(&mut self) {
        // No-op. Use Device::flush_to_disk() explicitly before drop.
    }
}

impl Device {
    /// Persist all durable state on shutdown: battery-backed cartridge RAM
    /// (best-effort) and then the auto-save snapshot. Idempotent; safe to call
    /// from non-Drop contexts. The battery save is best-effort because it is
    /// already retried on every dirty frame by the emulator loop; only an
    /// auto-save IO failure is surfaced as Err.
    pub fn flush_to_disk(&self) -> StrResult<()> {
        // No-op when the cart isn't battery-backed or has no save path.
        let _ = self.save_battery_ram_silent();

        let Some(path) = &self.save_state else {
            return Ok(());
        };
        let data = encode_cpu_state(&self.cpu)?;
        write_atomic(std::path::Path::new(path), &data)
            .map_err(|_| "Failed to write auto-save state")
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn test_rom() -> Vec<u8> {
        let mut rom = vec![0; 0x8000];
        rom[0x134..0x13C].copy_from_slice(b"RKYVTEST");
        rom[0x147] = 0x00;
        rom
    }

    #[test]
    fn v4_magic_is_used_for_writes() {
        let cart = mbc::Cartridge::from_buffer(test_rom(), true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();
        let data = encode_cpu_state(&cpu).unwrap();
        assert!(data.starts_with(b"RGBEST04"), "writes must use V4 magic");
    }

    #[test]
    fn old_format_saves_are_rejected() {
        for magic in [b"RGBEST01", b"RGBEST02", b"RGBEST03"] {
            let mut fake = Vec::from(magic as &[u8]);
            fake.extend_from_slice(&[0u8; 64]);
            assert!(
                decode_cpu_state(&fake).is_err(),
                "{} saves must not load",
                std::str::from_utf8(magic).unwrap()
            );
        }
    }

    #[test]
    fn cpu_state_round_trips_with_rkyv_header() {
        let cart = mbc::Cartridge::from_buffer(test_rom(), true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();

        let data = encode_cpu_state(&cpu).unwrap();
        assert!(data.starts_with(SAVE_STATE_MAGIC));

        let mut decoded = decode_cpu_state(&data).unwrap();
        decoded.mmu.mbc.set_rom(test_rom());
        assert_eq!(decoded.mmu.mbc.romname(), "RKYVTEST");
        assert!(decoded.mmu.sound.is_none());
    }

    #[test]
    fn cpu_state_round_trips_with_v4_preview() {
        let cart = mbc::Cartridge::from_buffer(test_rom(), true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();
        let thumbnail = vec![7; crate::gpu::SCREEN_W * crate::gpu::SCREEN_H * 3];

        let (data, preview) = encode_cpu_state_with_preview(&cpu, Some(&thumbnail)).unwrap();
        assert!(data.starts_with(SAVE_STATE_MAGIC));
        assert_eq!(preview.thumbnail_width, crate::gpu::SCREEN_W as u16);
        assert_eq!(preview.thumbnail_height, crate::gpu::SCREEN_H as u16);
        assert_eq!(preview.thumbnail_rgb.as_deref(), Some(thumbnail.as_slice()));

        let mut decoded = decode_cpu_state(&data).unwrap();
        decoded.mmu.mbc.set_rom(test_rom());
        assert_eq!(decoded.mmu.mbc.romname(), "RKYVTEST");
        assert!(decoded.mmu.sound.is_none());

        let preview_path = std::env::temp_dir().join(format!(
            "rust_gbe_v2_preview_test_{}.sav",
            std::process::id()
        ));
        std::fs::write(&preview_path, &data).unwrap();
        let parsed_preview = read_save_state_preview(&preview_path).unwrap();
        assert_eq!(parsed_preview, preview);
        let _ = std::fs::remove_file(preview_path);
    }

    #[test]
    fn slot_save_writes_v4_preview_file() {
        let cart = mbc::Cartridge::from_buffer(test_rom(), true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();
        let save_dir =
            std::env::temp_dir().join(format!("rust_gbe_v2_slot_save_test_{}", std::process::id()));
        std::fs::create_dir_all(&save_dir).unwrap();
        let device = Device {
            cpu,
            save_state: Some(save_dir.join("game.state").to_string_lossy().to_string()),
        };
        let thumbnail = vec![9; crate::gpu::SCREEN_W * crate::gpu::SCREEN_H * 3];

        let preview = device.save_state_slot(4, Some(&thumbnail)).unwrap();
        let slot_path = save_dir.join("save_state_4.sav");
        let data = std::fs::read(&slot_path).unwrap();
        assert!(data.starts_with(SAVE_STATE_MAGIC));
        assert_eq!(read_save_state_preview(&slot_path).unwrap(), preview);
        assert_eq!(preview.thumbnail_rgb.as_deref(), Some(thumbnail.as_slice()));

        let mut decoded = decode_cpu_state(&data).unwrap();
        decoded.mmu.mbc.set_rom(test_rom());
        assert_eq!(decoded.mmu.mbc.romname(), "RKYVTEST");

        let _ = std::fs::remove_file(slot_path);
        let _ = std::fs::remove_dir(save_dir);
    }

    #[test]
    fn rejects_unversioned_save_state_data() {
        let err = match decode_cpu_state(b"not a rkyv save state") {
            Ok(_) => panic!("unversioned data should not decode"),
            Err(err) => err,
        };
        assert_eq!(err, "Unsupported save state format");
    }

    #[test]
    fn thumbnailless_save_preview_has_header_timestamp_and_no_thumbnail() {
        let cart = mbc::Cartridge::from_buffer(test_rom(), true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();
        let data = encode_cpu_state(&cpu).unwrap();
        let preview_path = std::env::temp_dir().join(format!(
            "rust_gbe_thumbnailless_preview_test_{}.sav",
            std::process::id()
        ));
        std::fs::write(&preview_path, &data).unwrap();

        let preview = read_save_state_preview(&preview_path).unwrap();
        assert!(preview.saved_at_unix_secs > 0);
        assert_eq!(preview.thumbnail_width, 0);
        assert_eq!(preview.thumbnail_height, 0);
        assert!(preview.thumbnail_rgb.is_none());

        let _ = std::fs::remove_file(preview_path);
    }

    #[test]
    fn preview_reader_rejects_missing_and_malformed_files() {
        let missing_path = std::env::temp_dir().join(format!(
            "rust_gbe_missing_preview_test_{}.sav",
            std::process::id()
        ));
        assert!(read_save_state_preview(&missing_path).is_none());

        let malformed_path = std::env::temp_dir().join(format!(
            "rust_gbe_malformed_preview_test_{}.sav",
            std::process::id()
        ));
        std::fs::write(&malformed_path, SAVE_STATE_MAGIC).unwrap();
        assert!(read_save_state_preview(&malformed_path).is_none());
        let err = match decode_cpu_state(&std::fs::read(&malformed_path).unwrap()) {
            Ok(_) => panic!("malformed v3 data should not decode"),
            Err(err) => err,
        };
        assert_eq!(err, "Failed to parse save state");

        let _ = std::fs::remove_file(malformed_path);
    }

    #[test]
    fn file_backed_save_path_survives_round_trip() {
        let rom_path =
            std::env::temp_dir().join(format!("rust_gbe_rkyv_test_{}.gb", std::process::id()));
        std::fs::write(&rom_path, test_rom()).unwrap();

        let expected_save_path = rom_path
            .with_extension("gbsave")
            .to_string_lossy()
            .to_string();
        let cart = mbc::Cartridge::from_file(rom_path.clone(), true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();
        let data = encode_cpu_state(&cpu).unwrap();
        let decoded = decode_cpu_state(&data).unwrap();

        assert_eq!(
            decoded.mmu.mbc.get_save_path().as_deref(),
            Some(expected_save_path.as_str())
        );

        let _ = std::fs::remove_file(rom_path);
        let _ = std::fs::remove_file(expected_save_path);
    }

    #[test]
    fn flush_to_disk_writes_auto_save_and_handles_battery_ram() {
        let mut rom = vec![0; 0x8000];
        rom[0x134..0x13C].copy_from_slice(b"FLUSHTST");
        rom[0x147] = 0x03; // MBC1 + RAM + BATTERY
        rom[0x149] = 0x02; // 8 KiB RAM
        let cart = mbc::Cartridge::from_buffer(rom, true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();
        let save_dir = std::env::temp_dir()
            .join(format!("rust_gbe_flush_battery_test_{}", std::process::id()));
        std::fs::create_dir_all(&save_dir).unwrap();
        let state_path = save_dir.join("game.state");
        let device = Device {
            cpu,
            save_state: Some(state_path.to_string_lossy().to_string()),
        };

        assert!(device.flush_to_disk().is_ok());
        // The auto-save snapshot must exist and decode cleanly (current format).
        let data = std::fs::read(&state_path).unwrap();
        assert!(data.starts_with(SAVE_STATE_MAGIC));
        assert!(decode_cpu_state(&data).is_ok());

        let _ = std::fs::remove_dir_all(&save_dir);
    }

    #[test]
    fn slot_save_path_uses_save_state_directory() {
        let cart = mbc::Cartridge::from_buffer(test_rom(), true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();
        let save_dir =
            std::env::temp_dir().join(format!("rust_gbe_slot_path_test_{}", std::process::id()));
        let device = Device {
            cpu,
            save_state: Some(save_dir.join("game.state").to_string_lossy().to_string()),
        };

        assert_eq!(
            device.save_state_slot_path(3),
            save_dir.join("save_state_3.sav")
        );
    }

    #[test]
    fn rom_is_excluded_from_serialized_payload() {
        let cart = mbc::Cartridge::from_buffer(test_rom(), true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();
        let data = encode_cpu_state(&cpu).unwrap();
        // test_rom() puts "RKYVTEST" at 0x134; if the ROM were serialized those
        // bytes would appear in the payload.
        assert!(
            !data.windows(8).any(|w| w == b"RKYVTEST"),
            "ROM bytes must not appear in a rom-skipped payload"
        );
    }

    #[test]
    fn auto_save_round_trips_via_load_auto_save() {
        let cart = mbc::Cartridge::from_buffer(test_rom(), true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();
        let save_dir = std::env::temp_dir()
            .join(format!("rust_gbe_auto_resume_test_{}", std::process::id()));
        std::fs::create_dir_all(&save_dir).unwrap();
        let mut device = Device {
            cpu,
            save_state: Some(save_dir.join("game.state").to_string_lossy().to_string()),
        };

        assert!(!device.has_auto_save(), "no auto-save before first flush");
        device.write_byte(0xC000, 0x5A);
        device.flush_to_disk().unwrap();
        assert!(device.has_auto_save());
        device.write_byte(0xC000, 0x00);

        device.load_auto_save().unwrap();
        assert_eq!(device.read_byte(0xC000), 0x5A, "WRAM restored from auto-save");
        assert_eq!(device.romname(), "RKYVTEST", "ROM carried across restore");

        let _ = std::fs::remove_dir_all(save_dir);
    }

    #[test]
    fn load_auto_save_errors_without_file_or_path() {
        let cart = mbc::Cartridge::from_buffer(test_rom(), true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();
        let missing = std::env::temp_dir()
            .join(format!("rust_gbe_missing_auto_{}.state", std::process::id()));
        let mut device = Device {
            cpu,
            save_state: Some(missing.to_string_lossy().to_string()),
        };
        assert!(!device.has_auto_save());
        assert!(device.load_auto_save().is_err());

        let cart = mbc::Cartridge::from_buffer(test_rom(), true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();
        let mut device = Device { cpu, save_state: None };
        assert!(!device.has_auto_save());
        assert!(device.load_auto_save().is_err());
    }

    #[test]
    fn rewind_snapshot_restore_round_trip() {
        let cart = mbc::Cartridge::from_buffer(test_rom(), true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();
        let mut device = Device { cpu, save_state: None };

        device.write_byte(0xC000, 0xAB); // WRAM
        let snap = device.snapshot_rewind().unwrap();
        device.write_byte(0xC000, 0xCD);
        assert_eq!(device.read_byte(0xC000), 0xCD);

        device.restore_rewind(&snap).unwrap();
        assert_eq!(device.read_byte(0xC000), 0xAB, "WRAM restored from snapshot");
        assert_eq!(device.romname(), "RKYVTEST", "ROM readable after restore");
    }

    #[test]
    fn slot_save_path_falls_back_to_file_backed_rom_directory() {
        let rom_dir =
            std::env::temp_dir().join(format!("rust_gbe_slot_rom_dir_test_{}", std::process::id()));
        std::fs::create_dir_all(&rom_dir).unwrap();
        let rom_path = rom_dir.join("game.gb");
        std::fs::write(&rom_path, test_rom()).unwrap();

        let cart = mbc::Cartridge::from_file(rom_path.clone(), true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();
        let device = Device {
            cpu,
            save_state: None,
        };

        assert_eq!(
            device.save_state_slot_path(2),
            rom_dir.join("save_state_2.sav")
        );

        let _ = std::fs::remove_file(rom_path);
        let _ = std::fs::remove_file(rom_dir.join("game.gbsave"));
        let _ = std::fs::remove_dir(rom_dir);
    }
}

impl Device {
    pub fn new(
        romname: &str,
        skip_checksum: bool,
        save_state: Option<String>,
    ) -> StrResult<Device> {
        let cart = mbc::Cartridge::from_file(romname.into(), skip_checksum)?;
        CPU::new(cart, None).map(|cpu| Device {
            cpu: cpu,
            save_state,
        })
    }

    pub fn new_cgb(
        romname: &str,
        skip_checksum: bool,
        save_state: Option<String>,
    ) -> StrResult<Device> {
        let cart = mbc::Cartridge::from_file(romname.into(), skip_checksum)?;
        CPU::new_cgb(cart, None).map(|cpu| Device {
            cpu: cpu,
            save_state,
        })
    }

    pub fn new_from_buffer(
        romdata: Vec<u8>,
        skip_checksum: bool,
        save_state: Option<String>,
    ) -> StrResult<Device> {
        let cart = mbc::Cartridge::from_buffer(romdata, skip_checksum)?;
        CPU::new(cart, None).map(|cpu| Device {
            cpu: cpu,
            save_state,
        })
    }

    pub fn new_cgb_from_buffer(
        romdata: Vec<u8>,
        skip_checksum: bool,
        save_state: Option<String>,
    ) -> StrResult<Device> {
        let cart = mbc::Cartridge::from_buffer(romdata, skip_checksum)?;
        CPU::new_cgb(cart, None).map(|cpu| Device {
            cpu: cpu,
            save_state,
        })
    }

    pub fn do_cycle(&mut self) -> u32 {
        self.cpu.do_cycle()
    }

    pub fn check_and_reset_gpu_updated(&mut self) -> bool {
        let result = self.cpu.mmu.gpu.updated;
        self.cpu.mmu.gpu.updated = false;
        result
    }

    pub fn get_gpu_data(&self) -> &[u8] {
        self.cpu.mmu.gpu.front_buffer()
    }

    pub fn enable_audio(&mut self, player: Box<dyn apu::AudioPlayer>, is_on: bool) {
        match self.cpu.mmu.gbmode {
            GbMode::Classic => {
                self.cpu.mmu.sound = Some(apu::Sound::new_dmg(player));
            }
            GbMode::Color | GbMode::ColorAsClassic => {
                self.cpu.mmu.sound = Some(apu::Sound::new_cgb(player));
            }
        };
        if is_on {
            if let Some(sound) = self.cpu.mmu.sound.as_mut() {
                sound.set_on();
            }
        }
    }

    pub fn set_master_volume(&mut self, v: f32) {
        if let Some(sound) = self.cpu.mmu.sound.as_mut() {
            sound.set_master_volume(v);
        }
    }

    pub fn sync_audio(&mut self) {
        if let Some(ref mut sound) = self.cpu.mmu.sound {
            sound.sync();
        }
    }

    pub fn keyup(&mut self, key: KeypadKey) {
        self.cpu.mmu.keypad.keyup(key);
    }

    pub fn keydown(&mut self, key: KeypadKey) {
        self.cpu.mmu.keypad.keydown(key);
    }

    pub fn romname(&self) -> String {
        self.cpu.mmu.mbc.romname()
    }

    pub fn is_cgb_mode(&self) -> bool {
        self.cpu.mmu.gbmode == GbMode::Color
    }

    pub fn loadram(&mut self, ramdata: &[u8]) -> StrResult<()> {
        self.cpu.mmu.mbc.loadram(ramdata)
    }

    pub fn dumpram(&self) -> Vec<u8> {
        self.cpu.mmu.mbc.dumpram()
    }

    pub fn ram_is_battery_backed(&self) -> bool {
        self.cpu.mmu.mbc.is_battery_backed()
    }

    pub fn check_and_reset_ram_updated(&mut self) -> bool {
        self.cpu.mmu.mbc.check_and_reset_ram_updated()
    }

    pub fn save_battery_ram(&self) -> StrResult<()> {
        self.save_battery_ram_with_message(true)
    }

    pub fn save_battery_ram_silent(&self) -> StrResult<()> {
        self.save_battery_ram_with_message(false)
    }

    fn save_battery_ram_with_message(&self, show_message: bool) -> StrResult<()> {
        if !self.cpu.mmu.mbc.is_battery_backed() {
            return Ok(());
        }
        let ram_data = self.cpu.mmu.mbc.dumpram();
        let Some(save_path) = self.cpu.mmu.mbc.get_save_path() else {
            if show_message {
                eprintln!("No save path available from MBC");
            }
            return Err("No save path available");
        };
        match write_atomic(std::path::Path::new(&save_path), &ram_data) {
            Ok(_) => {
                if show_message {
                    println!("Game save written to {} ({} bytes)", save_path, ram_data.len());
                }
                Ok(())
            }
            Err(e) => {
                eprintln!("Failed to write game save to {}: {}", save_path, e);
                Err("Failed to write game save")
            }
        }
    }

    pub fn read_byte(&mut self, address: u16) -> u8 {
        self.cpu.read_byte(address)
    }
    pub fn write_byte(&mut self, address: u16, byte: u8) {
        self.cpu.write_byte(address, byte)
    }
    pub fn read_wide(&mut self, address: u16) -> u16 {
        self.cpu.read_wide(address)
    }
    pub fn write_wide(&mut self, address: u16, byte: u16) {
        self.cpu.write_wide(address, byte)
    }

    pub fn save_state_slot_path(&self, slot: u8) -> PathBuf {
        let filename = format!("save_state_{}.sav", slot);

        if let Some(path) = self.save_state.as_deref().and_then(parent_dir) {
            return path.join(filename);
        }

        if let Some(path) = self
            .cpu
            .mmu
            .mbc
            .get_save_path()
            .as_deref()
            .and_then(parent_dir)
        {
            return path.join(filename);
        }

        std::path::PathBuf::from(filename)
    }

    pub fn save_state_slot(
        &self,
        slot: u8,
        thumbnail_rgb: Option<&[u8]>,
    ) -> StrResult<SaveStatePreview> {
        println!("Saving state to slot {}...", slot);

        let (serialized_data, preview) =
            match encode_cpu_state_with_preview(&self.cpu, thumbnail_rgb) {
                Ok(result) => result,
                Err(e) => {
                    eprintln!("Failed to serialize CPU state for slot {}", slot);
                    return Err(e);
                }
            };

        let save_path = self.save_state_slot_path(slot);

        match write_atomic(&save_path, &serialized_data) {
            Ok(_) => {
                println!("State saved to slot {} ({})", slot, save_path.display());
                Ok(preview)
            }
            Err(_) => {
                eprintln!("Failed to write save state file for slot {}", slot);
                Err("Failed to write save state file")
            }
        }
    }

    /// Serialize current mutable state for the rewind ring buffer. The ROM is
    /// excluded (see the rom-skip in the MBCs); the APU is excluded as usual.
    /// Returns the raw rkyv payload — no disk header, since these never hit disk.
    pub fn snapshot_rewind(&self) -> StrResult<Vec<u8>> {
        rkyv::to_bytes::<rkyv::rancor::Error>(&self.cpu)
            .map(|bytes| bytes.to_vec())
            .map_err(|_| "Failed to serialize rewind snapshot")
    }

    /// Restore a snapshot produced by `snapshot_rewind`, carrying the live ROM
    /// and audio player across the swap.
    pub fn restore_rewind(&mut self, payload: &[u8]) -> StrResult<()> {
        let decoded = rkyv::from_bytes::<CPU, rkyv::rancor::Error>(payload)
            .map_err(|_| "Failed to deserialize rewind snapshot")?;
        self.install_decoded_cpu(decoded);
        Ok(())
    }

    /// Install a freshly-decoded (rom-less, audio-less) CPU as the live CPU,
    /// carrying the immutable ROM and the live audio player across the swap.
    /// Shared by slot-load and rewind restore.
    fn install_decoded_cpu(&mut self, mut decoded: CPU) {
        let rom = self.cpu.mmu.mbc.take_rom(); // zero-copy move from outgoing CPU
        decoded.mmu.mbc.set_rom(rom);
        decoded.mmu.sound = self.cpu.mmu.sound.take(); // keep the cpal player alive
        self.cpu = decoded;
        self.sync_audio(); // drop any stale queued samples
    }

    /// True when the auto-save path is configured and the file exists on disk.
    pub fn has_auto_save(&self) -> bool {
        self.save_state
            .as_deref()
            .is_some_and(|p| std::path::Path::new(p).is_file())
    }

    /// Load the auto-save snapshot written by `flush_to_disk`, carrying the
    /// live ROM and audio player across the swap (same path as slot loads).
    pub fn load_auto_save(&mut self) -> StrResult<()> {
        let Some(path) = self.save_state.as_deref() else {
            return Err("No auto-save path configured");
        };
        let data = std::fs::read(path).map_err(|_| "No auto-save file")?;
        let cpu = decode_cpu_state(&data)?;
        self.install_decoded_cpu(cpu);
        Ok(())
    }

    pub fn load_state_slot(&mut self, slot: u8) -> StrResult<()> {
        println!("Loading state from slot {}...", slot);
        let save_path = self.save_state_slot_path(slot);

        match std::fs::read(&save_path) {
            Ok(data) => match decode_cpu_state(&data) {
                Ok(cpu) => {
                    self.install_decoded_cpu(cpu);
                    println!("State loaded from slot {}", slot);
                    Ok(())
                }
                Err(e) => {
                    eprintln!(
                        "Failed to parse save state from slot {} (file may be corrupted)",
                        slot
                    );
                    Err(e)
                }
            },
            Err(_) => {
                eprintln!("Save state slot {} does not exist", slot);
                Err("Save state file does not exist")
            }
        }
    }
}
