// Sharp SM83 CPU Registers

// Registers
#[derive(Debug)]
pub struct Registers {
    pub a: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub f: u8, // Flags
    pub h: u8,
    pub l: u8,
    pub sp: u16, // Stack pointer
    pub pc: u16, // Program counter
}

// Flags
#[derive(Debug)]
pub enum Flag {
    Z = 0b1000_0000, // Zero
    N = 0b0100_0000, // Subtract
    H = 0b0010_0000, // Half-carry
    C = 0b0001_0000, // Carry
}

impl Registers {
    pub fn new() -> Self {
        Registers {
            a: 0, b: 0, c: 0, d: 0,
            e: 0, f:0, h: 0, l: 0,
            sp: 0, pc: 0,
        }
    }

    // 16-bit register pair operations
    pub fn get_af(&self) -> u16 {
        ((self.a as u16) << 8) | (self.f as u16)
    }

    pub fn set_af(&mut self, value: u16) {
        self.a = (value >> 8) as u8;
        self.f = (value & 0xF0) as u8; // Bits 0-3 of F must be grounded to 0
    }

    pub fn get_bc(&self) -> u16 {
        ((self.b as u16) << 8) | (self.c as u16)
    }

    pub fn set_bc(&mut self, value: u16) {
        self.b = (value >> 8) as u8;
        self.c = (value & 0xFF) as u8;
    }

    pub fn get_de(&self) -> u16 {
        ((self.d as u16) << 8) | (self.e as u16)
    }

    pub fn set_de(&mut self, value: u16) {
        self.d = (value >> 8) as u8;
        self.e = (value & 0xFF) as u8;
    }

    pub fn get_hl(&self) -> u16 {
        ((self.h as u16) << 8) | (self.l as u16)
    }

    pub fn set_hl(&mut self, value: u16) {
        self.h = (value >> 8) as u8;
        self.l = (value & 0xFF) as u8;
    }

    // Flag operations
    pub fn get_flag(&self, flag: Flag) -> bool {
        (self.f & (flag as u8)) != 0
    }

    pub fn set_flag(&mut self, flag: Flag, value: bool) {
        if value {
            self.f |= flag as u8;
        } else {
            self.f &= !(flag as u8);
        }
    }
}