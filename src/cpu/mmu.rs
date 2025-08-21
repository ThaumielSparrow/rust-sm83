// Memory Management Unit
// Holds memory regions and basic MBC handling.

#[derive(Debug, Clone, Copy)]
pub enum MBCType {
    None,
    MBC1,
    MBC2,
    MBC3,
    MBC5,
}

pub struct Memory {
    // Memory regions
    pub rom_bank_0: [u8; 0x4000],    // 0x0000-0x3FFF - ROM Bank 0 (fixed)
    pub rom_bank_n: [u8; 0x4000],    // 0x4000-0x7FFF - ROM Bank 1-N (switchable)
    pub vram: [u8; 0x2000],          // 0x8000-0x9FFF - Video RAM
    pub external_ram: [u8; 0x2000],  // 0xA000-0xBFFF - External RAM
    pub wram: [u8; 0x2000],          // 0xC000-0xDFFF - Work RAM
    pub echo_ram: [u8; 0x1E00],      // 0xE000-0xFDFF - Echo of Work RAM
    pub oam: [u8; 0xA0],             // 0xFE00-0xFE9F - Object Attribute Memory
    pub unusable: [u8; 0x60],        // 0xFEA0-0xFEFF - Unusable
    pub io_registers: [u8; 0x80],    // 0xFF00-0xFF7F - I/O Registers
    pub hram: [u8; 0x7F],            // 0xFF80-0xFFFE - High RAM
    pub interrupt_enable: u8,        // 0xFFFF - Interrupt Enable Register

    // Memory bank controller state
    pub mbc_type: MBCType,
    pub rom_bank: usize,
    pub ram_bank: usize,
    pub ram_enabled: bool,
    pub banking_mode: u8,
}

impl Memory {
    pub fn new() -> Self {
        Memory {
            rom_bank_0: [0; 0x4000],
            rom_bank_n: [0; 0x4000],
            vram: [0; 0x2000],
            external_ram: [0; 0x2000],
            wram: [0; 0x2000],
            echo_ram: [0; 0x1E00],
            oam: [0; 0xA0],
            unusable: [0; 0x60],
            io_registers: [0; 0x80],
            hram: [0; 0x7F],
            interrupt_enable: 0,
            mbc_type: MBCType::None,
            rom_bank: 1,
            ram_bank: 0,
            ram_enabled: false,
            banking_mode: 0,
        }
    }


    pub fn read_byte(&self, addr: u16) -> u8 {
        match addr {
            // ROM Bank 0
            0x0000..=0x3FFF => self.rom_bank_0[addr as usize],

            // ROM Bank 1-N (switchable)
            0x4000..=0x7FFF => self.rom_bank_n[(addr - 0x4000) as usize],

            // Video RAM
            0x8000..=0x9FFF => self.vram[(addr - 0x8000) as usize],

            // External RAM (cartridge RAM)
            0xA000..=0xBFFF => {
                if self.ram_enabled {
                    self.external_ram[(addr - 0xA000) as usize]
                } else {
                    0xFF
                }
            }

            // Work RAM
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize],

            // Echo of Work RAM
            0xE000..=0xFDFF => self.wram[(addr - 0xE000) as usize],

            // Object Attribute Memory (OAM)
            0xFE00..=0xFE9F => self.oam[(addr - 0xFE00) as usize],

            // Unusable memory area
            0xFEA0..=0xFEFF => 0xFF,

            // I/O Registers
            0xFF00..=0xFF7F => self.read_io_register(addr),

            // High RAM
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],

            // Interrupt Enable Register
            0xFFFF => self.interrupt_enable,
        }
    }

    pub fn write_byte(&mut self, addr: u16, value: u8) {
        match addr {
            // ROM area - MBC register writes
            0x0000..=0x7FFF => self.write_mbc_register(addr, value),

            // Video RAM
            0x8000..=0x9FFF => self.vram[(addr - 0x8000) as usize] = value,

            // External RAM
            0xA000..=0xBFFF => {
                if self.ram_enabled {
                    self.external_ram[(addr - 0xA000) as usize] = value;
                }
            }

            // Work RAM
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize] = value,

            // Echo of Work RAM
            0xE000..=0xFDFF => self.wram[(addr - 0xE000) as usize] = value,

            // Object Attribute Memory (OAM)
            0xFE00..=0xFE9F => self.oam[(addr - 0xFE00) as usize] = value,

            // Unusable memory area
            0xFEA0..=0xFEFF => {} // Ignore writes

            // I/O Registers
            0xFF00..=0xFF7F => self.write_io_register(addr, value),

            // High RAM
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = value,

            // Interrupt Enable Register
            0xFFFF => self.interrupt_enable = value,
        }
    }

    fn read_io_register(&self, addr: u16) -> u8 {
        match addr {
            // Joypad
            0xFF00 => self.io_registers[0x00],

            // Serial transfer
            0xFF01..=0xFF02 => self.io_registers[(addr - 0xFF00) as usize],

            // Timer
            0xFF04..=0xFF07 => self.io_registers[(addr - 0xFF00) as usize],

            // Interrupt Flag
            0xFF0F => self.io_registers[0x0F],

            // Sound registers
            0xFF10..=0xFF3F => self.io_registers[(addr - 0xFF00) as usize],


            // DMA register
            0xFF46 => self.io_registers[0x46],

            // LCD registers
            0xFF40..=0xFF4B => self.io_registers[(addr - 0xFF00) as usize],

            // Boot ROM disable
            0xFF50 => self.io_registers[0x50],

            _ => self.io_registers[(addr - 0xFF00) as usize],
        }
    }

    fn write_io_register(&mut self, addr: u16, value: u8) {
        match addr {
            // DMA transfer
            0xFF46 => {
                self.io_registers[0x46] = value;
                self.dma_transfer(value);
            }

            // All other I/O registers
            _ => self.io_registers[(addr - 0xFF00) as usize] = value,
        }
    }

    fn write_mbc_register(&mut self, addr: u16, value: u8) {
        match self.mbc_type {
            MBCType::None => {
                // No memory bank controller: allow direct writes to the ROM buffers
                match addr {
                    0x0000..=0x3FFF => {
                        self.rom_bank_0[addr as usize] = value;
                    }
                    0x4000..=0x7FFF => {
                        self.rom_bank_n[(addr - 0x4000) as usize] = value;
                    }
                    _ => {}
                }
            }

            MBCType::MBC1 => {
                match addr {
                    0x0000..=0x1FFF => {
                        // RAM Enable
                        self.set_ram_enabled((value & 0x0F) == 0x0A);
                    }
                    0x2000..=0x3FFF => {
                        // ROM Bank Number (lower 5 bits)
                        let mut bank = (value & 0x1F) as usize;
                        if bank == 0 { bank = 1; } // Bank 0 maps to 1
                        self.select_rom_bank((self.rom_bank & 0x60) | bank);
                    }
                    0x4000..=0x5FFF => {
                        // RAM Bank Number or ROM Bank Number (upper 2 bits)
                        let upper_bits = ((value & 0x03) as usize) << 5;
                        if self.banking_mode == 0 {
                            // ROM banking mode
                            self.select_rom_bank((self.rom_bank & 0x1F) | upper_bits);
                        } else {
                            // RAM banking mode
                            self.select_ram_bank((value & 0x03) as usize);
                        }
                    }
                    0x6000..=0x7FFF => {
                        // Banking Mode Select
                        self.set_banking_mode(value & 0x01);
                    }
                    _ => {}
                }
            }

            _ => {} // Other MBC types not implemented yet
        }
    }

    // Bank switching helpers - expose clearer APIs for MBC operations
    pub fn select_rom_bank(&mut self, bank: usize) {
        self.rom_bank = bank;
        // Note: If we had the full ROM data stored, we'd copy the selected
        // bank into `rom_bank_n` here. For now we only update the index.
    }

    pub fn select_ram_bank(&mut self, bank: usize) {
        self.ram_bank = bank;
    }

    pub fn set_ram_enabled(&mut self, enabled: bool) {
        self.ram_enabled = enabled;
    }

    pub fn set_banking_mode(&mut self, mode: u8) {
        self.banking_mode = mode & 0x01;
    }

    fn dma_transfer(&mut self, source: u8) {
        let source_addr = (source as u16) << 8;
        for i in 0..0xA0 {
            let byte = self.read_byte(source_addr + i);
            self.oam[i as usize] = byte;
        }
    }

    pub fn load_rom(&mut self, rom_data: &[u8]) {
        // Determine MBC type from cartridge header
        if rom_data.len() > 0x147 {
            self.mbc_type = match rom_data[0x147] {
                0x00 => MBCType::None,
                0x01..=0x03 => MBCType::MBC1,
                0x05..=0x06 => MBCType::MBC2,
                0x0F..=0x13 => MBCType::MBC3,
                0x19..=0x1E => MBCType::MBC5,
                _ => MBCType::None,
            };
        }

        // Load ROM Bank 0
        let bank_0_size = std::cmp::min(rom_data.len(), 0x4000);
        self.rom_bank_0[..bank_0_size].copy_from_slice(&rom_data[..bank_0_size]);

        // Load ROM Bank 1 (if available)
        if rom_data.len() > 0x4000 {
            let bank_1_size = std::cmp::min(rom_data.len() - 0x4000, 0x4000);
            self.rom_bank_n[..bank_1_size].copy_from_slice(&rom_data[0x4000..0x4000 + bank_1_size]);
        }
    }
}
