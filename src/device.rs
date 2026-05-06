use crate::cpu::CPU;
use crate::gbmode::GbMode;
use crate::keypad::KeypadKey;
use crate::mbc::{self, MBC};
use crate::apu;
use crate::StrResult;

pub struct Device {
    cpu: CPU,
    save_state: Option<String>,
}

const SAVE_STATE_MAGIC: &[u8; 8] = b"RGBEST01";

fn encode_cpu_state(cpu: &CPU) -> StrResult<Vec<u8>> {
    let payload = rkyv::to_bytes::<rkyv::rancor::Error>(cpu)
        .map_err(|_| "Failed to serialize CPU state")?;
    let mut data = Vec::with_capacity(SAVE_STATE_MAGIC.len() + payload.len());
    data.extend_from_slice(SAVE_STATE_MAGIC);
    data.extend_from_slice(&payload);
    Ok(data)
}

fn decode_cpu_state(data: &[u8]) -> StrResult<CPU> {
    let payload = data
        .strip_prefix(SAVE_STATE_MAGIC)
        .ok_or("Unsupported save state format")?;
    rkyv::from_bytes::<CPU, rkyv::rancor::Error>(payload)
        .map_err(|_| "Failed to parse save state")
}

fn parent_dir(path: &str) -> Option<std::path::PathBuf> {
    std::path::Path::new(path)
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(std::path::Path::to_path_buf)
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
                Ok(data) => { let _ = file.write_all(&data); }
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
        assert!(data.starts_with(SAVE_STATE_MAGIC));

        let decoded = decode_cpu_state(&data).unwrap();
        assert_eq!(decoded.mmu.mbc.romname(), "RKYVTEST");
        assert!(decoded.mmu.sound.is_none());
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
    fn file_backed_save_path_survives_round_trip() {
        let rom_path = std::env::temp_dir().join(format!(
            "rust_gbe_rkyv_test_{}.gb",
            std::process::id()
        ));
        std::fs::write(&rom_path, test_rom()).unwrap();

        let expected_save_path = rom_path.with_extension("gbsave").to_string_lossy().to_string();
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
        let save_dir = std::env::temp_dir().join(format!(
            "rust_gbe_slot_path_test_{}",
            std::process::id()
        ));
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
        let rom_dir = std::env::temp_dir().join(format!(
            "rust_gbe_slot_rom_dir_test_{}",
            std::process::id()
        ));
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
        if let Some(sound) = self.cpu.mmu.sound.as_mut() { sound.set_master_volume(v); }
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
                        let preview: Vec<String> = ram_data.iter().take(16).map(|b| format!("{:02X}", b)).collect();
                        println!("DEBUG: First 16 bytes of RAM: {}", preview.join(" "));
                    }
                }
                
                // Make the save completely asynchronous to prevent hanging
                std::thread::spawn(move || {
                    match std::fs::write(&save_path, &ram_data) {
                        Ok(_) => {
                            if show_message {
                                println!("Game save written to {} ({} bytes)", save_path, ram_data.len());
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to write game save to {}: {}", save_path, e);
                        }
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

    fn save_state_slot_path(&self, slot: u8) -> std::path::PathBuf {
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

    pub fn save_state_slot(&self, slot: u8) -> StrResult<()> {
        println!("Saving state to slot {}...", slot);

        let serialized_data = match encode_cpu_state(&self.cpu) {
            Ok(data) => data,
            Err(e) => {
                eprintln!("Failed to serialize CPU state for slot {}", slot);
                return Err(e);
            }
        };

        let save_path = self.save_state_slot_path(slot);

        // Write to file asynchronously to avoid blocking
        std::thread::spawn(move || {
            match std::fs::write(&save_path, &serialized_data) {
                Ok(_) => println!("State saved to slot {} ({})", slot, save_path.display()),
                Err(_) => eprintln!("Failed to write save state file for slot {}", slot),
            }
        });

        Ok(())
    }

    pub fn load_state_slot(&mut self, slot: u8) -> StrResult<()> {
        println!("Loading state from slot {}...", slot);
        let save_path = self.save_state_slot_path(slot);

        match std::fs::read(&save_path) {
            Ok(data) => {
                match decode_cpu_state(&data) {
                    Ok(cpu) => {
                        self.cpu = cpu;
                        println!("State loaded from slot {}", slot);
                        Ok(())
                    }
                    Err(e) => {
                        eprintln!("Failed to parse save state from slot {} (file may be corrupted)", slot);
                        Err(e)
                    }
                }
            }
            Err(_) => {
                eprintln!("Save state slot {} does not exist", slot);
                Err("Save state file does not exist")
            }
        }
    }
}
