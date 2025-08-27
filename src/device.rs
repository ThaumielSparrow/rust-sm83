use crate::cpu::CPU;
use crate::gbmode::GbMode;
use crate::keypad::KeypadKey;
use crate::mbc;
// Printer and external serial callback support removed.
use crate::sound;
use crate::StrResult;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Device {
    cpu: CPU,
    save_state: Option<String>,
}

impl Drop for Device {
    fn drop(&mut self) {
        if let Some(path) = &self.save_state {
            // Write final state to disk using bincode 2.0
            let mut file = match std::fs::File::create(path) {
                Ok(f) => f,
                Err(_) => return,
            };
            use std::io::Write;
            let config = bincode::config::standard().with_fixed_int_encoding();
            match bincode::serde::encode_to_vec(&self.cpu, config) {
                Ok(data) => { let _ = file.write_all(&data); }
                Err(_) => return,
            }
        }
    }
}

// StdoutPrinter & SerialCallback removed.

impl Device {
    pub fn load_state(path: &str) -> Option<Box<Device>> {
        let mut file = std::fs::File::open(path).ok()?;
        let mut data = Vec::new();
        use std::io::Read;
        if file.read_to_end(&mut data).is_err() {
            return None;
        }
        let config = bincode::config::standard().with_fixed_int_encoding();
        let cpu = bincode::serde::decode_from_slice::<CPU, _>(&data, config).ok()?.0;
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
        let cart = mbc::FileBackedMBC::new(romname.into(), skip_checksum)?;
        CPU::new(Box::new(cart), None).map(|cpu| Device {
            cpu: cpu,
            save_state,
        })
    }

    pub fn new_cgb(
        romname: &str,
        skip_checksum: bool,
        save_state: Option<String>,
    ) -> StrResult<Device> {
        let cart = mbc::FileBackedMBC::new(romname.into(), skip_checksum)?;
        CPU::new_cgb(Box::new(cart), None).map(|cpu| Device {
            cpu: cpu,
            save_state,
        })
    }

    pub fn new_from_buffer(
        romdata: Vec<u8>,
        skip_checksum: bool,
        save_state: Option<String>,
    ) -> StrResult<Device> {
        let cart = mbc::get_mbc(romdata, skip_checksum)?;
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
        let cart = mbc::get_mbc(romdata, skip_checksum)?;
        CPU::new_cgb(cart, None).map(|cpu| Device {
            cpu: cpu,
            save_state,
        })
    }

    pub fn do_cycle(&mut self) -> u32 {
        self.cpu.do_cycle()
    }

    // set_stdout / attach_printer / set_serial_callback removed.

    pub fn check_and_reset_gpu_updated(&mut self) -> bool {
        let result = self.cpu.mmu.gpu.updated;
        self.cpu.mmu.gpu.updated = false;
        result
    }

    pub fn get_gpu_data(&self) -> &[u8] {
        &self.cpu.mmu.gpu.data
    }

    pub fn enable_audio(&mut self, player: Box<dyn sound::AudioPlayer>, is_on: bool) {
        match self.cpu.mmu.gbmode {
            GbMode::Classic => {
                self.cpu.mmu.sound = Some(sound::Sound::new_dmg(player));
            }
            GbMode::Color | GbMode::ColorAsClassic => {
                self.cpu.mmu.sound = Some(sound::Sound::new_cgb(player));
            }
        };
        if is_on {
            if let Some(sound) = self.cpu.mmu.sound.as_mut() {
                sound.set_on();
            }
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

    pub fn save_state_slot(&self, slot: u8) -> StrResult<()> {
        println!("Saving state to slot {}...", slot);
        
        // Serialize to bytes in memory first using bincode 2.0
        let config = bincode::config::standard().with_fixed_int_encoding();
        let serialized_data = match bincode::serde::encode_to_vec(&self.cpu, config) {
            Ok(data) => data,
            Err(_) => {
                eprintln!("Failed to serialize CPU state for slot {}", slot);
                return Err("Failed to serialize CPU state");
            }
        };
        
        let save_path = format!("save_state_{}.sav", slot);
        
        // Write to file asynchronously to avoid blocking
        std::thread::spawn(move || {
            match std::fs::write(&save_path, &serialized_data) {
                Ok(_) => println!("State saved to slot {}", slot),
                Err(_) => eprintln!("Failed to write save state file for slot {}", slot),
            }
        });
        
        Ok(())
    }

    pub fn load_state_slot(&mut self, slot: u8) -> StrResult<()> {
        println!("Loading state from slot {}...", slot);
        let save_path = format!("save_state_{}.sav", slot);
        
        match std::fs::read(&save_path) {
            Ok(data) => {
                let config = bincode::config::standard().with_fixed_int_encoding();
                match bincode::serde::decode_from_slice::<crate::cpu::CPU, _>(&data, config) {
                    Ok((cpu, _)) => {
                        self.cpu = cpu;
                        println!("State loaded from slot {}", slot);
                        Ok(())
                    }
                    Err(_) => {
                        eprintln!("Failed to parse save state from slot {} (file may be corrupted)", slot);
                        Err("Failed to parse save state")
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
