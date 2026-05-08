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

const SAVE_STATE_MAGIC_V1: &[u8; 8] = b"RGBEST01";
const SAVE_STATE_MAGIC_V2: &[u8; 8] = b"RGBEST02";
const SAVE_STATE_V2_HEADER_LEN: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveStatePreview {
    pub saved_at_unix_secs: u64,
    pub thumbnail_width: u16,
    pub thumbnail_height: u16,
    pub thumbnail_rgb: Option<Vec<u8>>,
}

struct SaveStateV2Parts<'a> {
    cpu_payload: &'a [u8],
}

struct SaveStateV2Header {
    saved_at_unix_secs: u64,
    thumbnail_width: u16,
    thumbnail_height: u16,
    thumbnail_len: usize,
    cpu_payload_len: usize,
}

fn encode_cpu_state(cpu: &CPU) -> StrResult<Vec<u8>> {
    let payload =
        rkyv::to_bytes::<rkyv::rancor::Error>(cpu).map_err(|_| "Failed to serialize CPU state")?;
    let mut data = Vec::with_capacity(SAVE_STATE_MAGIC_V1.len() + payload.len());
    data.extend_from_slice(SAVE_STATE_MAGIC_V1);
    data.extend_from_slice(&payload);
    Ok(data)
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

    let mut data = Vec::with_capacity(SAVE_STATE_V2_HEADER_LEN + thumbnail_len + payload.len());
    data.extend_from_slice(SAVE_STATE_MAGIC_V2);
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
    let payload = if let Some(payload) = data.strip_prefix(SAVE_STATE_MAGIC_V1) {
        payload
    } else if data.starts_with(SAVE_STATE_MAGIC_V2) {
        parse_save_state_v2(data)
            .ok_or("Failed to parse save state")?
            .cpu_payload
    } else {
        return Err("Unsupported save state format");
    };

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

fn unix_secs_from_file_modified(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
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

fn parse_save_state_v2(data: &[u8]) -> Option<SaveStateV2Parts<'_>> {
    let header = parse_save_state_v2_header(data, data.len())?;

    let thumbnail_start = SAVE_STATE_V2_HEADER_LEN;
    let thumbnail_end = thumbnail_start.checked_add(header.thumbnail_len)?;
    let cpu_payload_end = thumbnail_end.checked_add(header.cpu_payload_len)?;

    if header.thumbnail_len != 0 {
        data.get(thumbnail_start..thumbnail_end)?;
    }
    let cpu_payload = data.get(thumbnail_end..cpu_payload_end)?;

    Some(SaveStateV2Parts { cpu_payload })
}

fn parse_save_state_v2_header(data: &[u8], total_len: usize) -> Option<SaveStateV2Header> {
    if !data.starts_with(SAVE_STATE_MAGIC_V2) || data.len() < SAVE_STATE_V2_HEADER_LEN {
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
        let expected_thumbnail_len = usize::from(thumbnail_width)
            .checked_mul(usize::from(thumbnail_height))?
            .checked_mul(3)?;
        if thumbnail_width == 0 || thumbnail_height == 0 || thumbnail_len != expected_thumbnail_len
        {
            return None;
        }
    }

    let expected_len = SAVE_STATE_V2_HEADER_LEN
        .checked_add(thumbnail_len)?
        .checked_add(cpu_payload_len)?;
    if total_len != expected_len {
        return None;
    }

    Some(SaveStateV2Header {
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

    if &magic == SAVE_STATE_MAGIC_V1 {
        return Some(SaveStatePreview {
            saved_at_unix_secs: unix_secs_from_file_modified(path),
            thumbnail_width: 0,
            thumbnail_height: 0,
            thumbnail_rgb: None,
        });
    }

    if &magic != SAVE_STATE_MAGIC_V2 {
        return None;
    }

    let total_len = usize::try_from(file.metadata().ok()?.len()).ok()?;
    let mut header_data = [0; SAVE_STATE_V2_HEADER_LEN];
    header_data[..SAVE_STATE_MAGIC_V2.len()].copy_from_slice(&magic);
    file.read_exact(&mut header_data[SAVE_STATE_MAGIC_V2.len()..])
        .ok()?;
    let header = parse_save_state_v2_header(&header_data, total_len)?;
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
        if let Some(path) = &self.save_state {
            let mut file = match std::fs::File::create(path) {
                Ok(f) => f,
                Err(_) => return,
            };
            use std::io::Write;
            match encode_cpu_state(&self.cpu) {
                Ok(data) => {
                    let _ = file.write_all(&data);
                }
                Err(_) => return,
            }
        }
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
    fn cpu_state_round_trips_with_rkyv_header() {
        let cart = mbc::Cartridge::from_buffer(test_rom(), true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();

        let data = encode_cpu_state(&cpu).unwrap();
        assert!(data.starts_with(SAVE_STATE_MAGIC_V1));

        let decoded = decode_cpu_state(&data).unwrap();
        assert_eq!(decoded.mmu.mbc.romname(), "RKYVTEST");
        assert!(decoded.mmu.sound.is_none());
    }

    #[test]
    fn cpu_state_round_trips_with_v2_preview() {
        let cart = mbc::Cartridge::from_buffer(test_rom(), true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();
        let thumbnail = vec![7; crate::gpu::SCREEN_W * crate::gpu::SCREEN_H * 3];

        let (data, preview) = encode_cpu_state_with_preview(&cpu, Some(&thumbnail)).unwrap();
        assert!(data.starts_with(SAVE_STATE_MAGIC_V2));
        assert_eq!(preview.thumbnail_width, crate::gpu::SCREEN_W as u16);
        assert_eq!(preview.thumbnail_height, crate::gpu::SCREEN_H as u16);
        assert_eq!(preview.thumbnail_rgb.as_deref(), Some(thumbnail.as_slice()));

        let decoded = decode_cpu_state(&data).unwrap();
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
    fn slot_save_writes_v2_preview_file() {
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
        assert!(data.starts_with(SAVE_STATE_MAGIC_V2));
        assert_eq!(read_save_state_preview(&slot_path).unwrap(), preview);
        assert_eq!(preview.thumbnail_rgb.as_deref(), Some(thumbnail.as_slice()));

        let decoded = decode_cpu_state(&data).unwrap();
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
    fn v1_preview_uses_file_modified_time_without_thumbnail() {
        let cart = mbc::Cartridge::from_buffer(test_rom(), true).unwrap();
        let cpu = CPU::new(cart, None).unwrap();
        let data = encode_cpu_state(&cpu).unwrap();
        let preview_path = std::env::temp_dir().join(format!(
            "rust_gbe_v1_preview_test_{}.sav",
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
        std::fs::write(&malformed_path, SAVE_STATE_MAGIC_V2).unwrap();
        assert!(read_save_state_preview(&malformed_path).is_none());
        let err = match decode_cpu_state(&std::fs::read(&malformed_path).unwrap()) {
            Ok(_) => panic!("malformed v2 data should not decode"),
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
    pub fn load_state(path: &str) -> Option<Box<Device>> {
        let mut file = std::fs::File::open(path).ok()?;
        let mut data = Vec::new();
        use std::io::Read;
        if file.read_to_end(&mut data).is_err() {
            return None;
        }
        let cpu = decode_cpu_state(&data).ok()?;
        Some(Box::new(Device {
            cpu,
            save_state: Some(path.to_string()),
        }))
    }

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

    pub fn check_ram_updated_status(&self) -> bool {
        // We need to add a method to check without resetting
        // For now, let's add debug info to the save function
        true // placeholder
    }

    pub fn save_battery_ram(&self) -> StrResult<()> {
        self.save_battery_ram_with_message(true)
    }

    pub fn save_battery_ram_silent(&self) -> StrResult<()> {
        self.save_battery_ram_with_message(false)
    }

    fn save_battery_ram_with_message(&self, show_message: bool) -> StrResult<()> {
        if self.cpu.mmu.mbc.is_battery_backed() {
            let ram_data = self.cpu.mmu.mbc.dumpram();

            if let Some(save_path) = self.cpu.mmu.mbc.get_save_path() {
                if show_message {
                    println!("DEBUG: Attempting to save to path: {}", save_path);
                    println!("DEBUG: RAM data size: {} bytes", ram_data.len());

                    // Show first 16 bytes of RAM for debugging
                    if ram_data.len() > 0 {
                        let preview: Vec<String> = ram_data
                            .iter()
                            .take(16)
                            .map(|b| format!("{:02X}", b))
                            .collect();
                        println!("DEBUG: First 16 bytes of RAM: {}", preview.join(" "));
                    }
                }

                // Make the save completely asynchronous to prevent hanging
                std::thread::spawn(move || match std::fs::write(&save_path, &ram_data) {
                    Ok(_) => {
                        if show_message {
                            println!(
                                "Game save written to {} ({} bytes)",
                                save_path,
                                ram_data.len()
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to write game save to {}: {}", save_path, e);
                    }
                });
                Ok(())
            } else {
                if show_message {
                    eprintln!("DEBUG: No save path available from MBC");
                }
                Err("No save path available")
            }
        } else {
            if show_message {
                println!("DEBUG: MBC is not battery-backed, no save needed");
            }
            Ok(()) // No battery-backed RAM, nothing to save
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

        match std::fs::write(&save_path, &serialized_data) {
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

    pub fn load_state_slot(&mut self, slot: u8) -> StrResult<()> {
        println!("Loading state from slot {}...", slot);
        let save_path = self.save_state_slot_path(slot);

        match std::fs::read(&save_path) {
            Ok(data) => match decode_cpu_state(&data) {
                Ok(cpu) => {
                    self.cpu = cpu;
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
