//! MBC7: ROM banking, a two-axis accelerometer and a 93LC56 serial EEPROM
//! (256 bytes, the battery save). Used by Kirby Tilt 'n' Tumble and
//! Command Master. Register layout per Pan Docs "MBC7".
use crate::mbc::{rom_banks, MBC};
use crate::StrResult;

/// Accelerometer reading at rest, and the change for 1 g of tilt.
const ACCEL_CENTER: u16 = 0x81D0;
const ACCEL_1G: f32 = 0x70 as f32;

#[derive(Clone, Copy, PartialEq, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
enum EepromState {
    /// Waiting for a start bit (DI high on a clock rising edge).
    Idle,
    /// Shifting in 2 opcode bits + 8 address bits.
    Command,
    /// WRITE / WRAL: shifting in 16 data bits.
    Data,
    /// READ: shifting data out on DO.
    Reading,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MBC7 {
    #[rkyv(with = rkyv::with::Skip)]
    rom: Vec<u8>,
    rombank: usize,
    rombanks: usize,
    ram_enable1: bool,
    ram_enable2: bool,
    /// Latched accelerometer X/Y (0x8000 until the first latch).
    accel: [u16; 2],
    /// Latch sequence: 0x55 to Ax0x arms, 0xAA to Ax1x latches.
    latch_armed: bool,
    /// Current tilt from the host, in g (-1..1 per axis). Input, not state.
    #[rkyv(with = rkyv::with::Skip)]
    tilt: (f32, f32),
    eeprom: Vec<u8>,
    eeprom_dirty: bool,
    // 93LC56 serial interface.
    cs: bool,
    clk: bool,
    di: bool,
    do_bit: bool,
    state: EepromState,
    shift: u32,
    bits: u32,
    command: u32,
    write_enabled: bool,
}

impl MBC7 {
    pub fn new(data: Vec<u8>) -> StrResult<MBC7> {
        let rombanks = rom_banks(data[0x148]);
        if rombanks == 0 {
            return Err("Invalid ROM size in cartridge header");
        }
        Ok(MBC7 {
            rom: data,
            rombank: 1,
            rombanks,
            ram_enable1: false,
            ram_enable2: false,
            accel: [0x8000; 2],
            latch_armed: false,
            tilt: (0.0, 0.0),
            eeprom: vec![0xFF; 256],
            eeprom_dirty: false,
            cs: false,
            clk: false,
            di: false,
            do_bit: true,
            state: EepromState::Idle,
            shift: 0,
            bits: 0,
            command: 0,
            write_enabled: false,
        })
    }

    fn word(&self, addr: usize) -> u16 {
        let i = (addr & 0x7F) * 2;
        u16::from_be_bytes([self.eeprom[i], self.eeprom[i + 1]])
    }

    fn set_word(&mut self, addr: usize, v: u16) {
        let i = (addr & 0x7F) * 2;
        self.eeprom[i..i + 2].copy_from_slice(&v.to_be_bytes());
        self.eeprom_dirty = true;
    }

    /// A rising clock edge with CS high: advance the serial protocol.
    fn eeprom_clock(&mut self) {
        match self.state {
            EepromState::Idle => {
                if self.di {
                    self.state = EepromState::Command;
                    self.shift = 0;
                    self.bits = 0;
                }
            }
            EepromState::Command => {
                self.shift = (self.shift << 1) | self.di as u32;
                self.bits += 1;
                if self.bits == 10 {
                    self.command = self.shift;
                    self.run_command();
                }
            }
            EepromState::Data => {
                self.shift = (self.shift << 1) | self.di as u32;
                self.bits += 1;
                if self.bits == 16 {
                    let value = self.shift as u16;
                    let addr = (self.command & 0xFF) as usize;
                    if self.write_enabled {
                        if self.command >> 8 == 0b01 {
                            self.set_word(addr, value); // WRITE
                        } else {
                            for a in 0..128 {
                                self.set_word(a, value); // WRAL
                            }
                        }
                    }
                    self.do_bit = true; // ready (writes complete instantly)
                    self.state = EepromState::Idle;
                }
            }
            EepromState::Reading => {
                self.do_bit = self.shift & 0x8000 != 0;
                self.shift <<= 1;
                self.bits += 1;
                if self.bits == 16 {
                    self.state = EepromState::Idle;
                }
            }
        }
    }

    /// 10 command bits received: 2-bit opcode, 8-bit address (or sub-op).
    fn run_command(&mut self) {
        let addr = (self.command & 0xFF) as usize;
        match self.command >> 8 {
            0b10 => {
                // READ: a dummy 0, then the word MSB first on later clocks.
                self.shift = self.word(addr) as u32;
                self.bits = 0;
                self.do_bit = false;
                self.state = EepromState::Reading;
            }
            0b01 => {
                self.shift = 0;
                self.bits = 0;
                self.state = EepromState::Data;
            }
            0b11 => {
                // ERASE
                if self.write_enabled {
                    self.set_word(addr, 0xFFFF);
                }
                self.do_bit = true;
                self.state = EepromState::Idle;
            }
            _ => {
                // 00: sub-op in the top two address bits.
                match addr >> 6 {
                    0b11 => self.write_enabled = true, // EWEN
                    0b00 => self.write_enabled = false, // EWDS
                    0b10 => {
                        // ERAL
                        if self.write_enabled {
                            for a in 0..128 {
                                self.set_word(a, 0xFFFF);
                            }
                        }
                    }
                    _ => {
                        // WRAL: 16 data bits follow.
                        self.shift = 0;
                        self.bits = 0;
                        self.state = EepromState::Data;
                        return;
                    }
                }
                self.do_bit = true;
                self.state = EepromState::Idle;
            }
        }
    }

    fn write_eeprom_pins(&mut self, v: u8) {
        let cs = v & 0x80 != 0;
        let clk = v & 0x40 != 0;
        self.di = v & 0x02 != 0;
        if !cs {
            // Deselect aborts any command in progress.
            self.state = EepromState::Idle;
        } else if clk && !self.clk {
            self.eeprom_clock();
        }
        self.cs = cs;
        self.clk = clk;
    }

    fn ram_enabled(&self) -> bool {
        self.ram_enable1 && self.ram_enable2
    }
}

impl MBC for MBC7 {
    fn readrom(&self, a: u16) -> u8 {
        let bank = if a < 0x4000 { 0 } else { self.rombank };
        *self.rom.get((bank * 0x4000) | (a as usize & 0x3FFF)).unwrap_or(&0xFF)
    }

    fn readram(&self, a: u16) -> u8 {
        if !self.ram_enabled() || a >= 0xB000 {
            return 0xFF;
        }
        match (a >> 4) & 0x0F {
            0x2 => self.accel[0] as u8,
            0x3 => (self.accel[0] >> 8) as u8,
            0x4 => self.accel[1] as u8,
            0x5 => (self.accel[1] >> 8) as u8,
            0x6 => 0x00,
            0x8 => {
                (self.cs as u8) << 7
                    | (self.clk as u8) << 6
                    | (self.di as u8) << 1
                    | self.do_bit as u8
            }
            _ => 0xFF,
        }
    }

    fn writerom(&mut self, a: u16, v: u8) {
        match a {
            0x0000..=0x1FFF => self.ram_enable1 = v == 0x0A,
            0x2000..=0x3FFF => self.rombank = (v as usize & 0x7F) % self.rombanks,
            0x4000..=0x5FFF => self.ram_enable2 = v == 0x40,
            _ => {}
        }
    }

    fn writeram(&mut self, a: u16, v: u8) {
        if !self.ram_enabled() || a >= 0xB000 {
            return;
        }
        match (a >> 4) & 0x0F {
            0x0 => {
                if v == 0x55 {
                    self.latch_armed = true;
                    self.accel = [0x8000; 2];
                }
            }
            0x1 => {
                if v == 0xAA && self.latch_armed {
                    self.latch_armed = false;
                    let axis = |g: f32| {
                        (ACCEL_CENTER as f32 + g.clamp(-1.5, 1.5) * ACCEL_1G) as u16
                    };
                    self.accel = [axis(self.tilt.0), axis(self.tilt.1)];
                }
            }
            0x8 => self.write_eeprom_pins(v),
            _ => {}
        }
    }

    fn check_and_reset_ram_updated(&mut self) -> bool {
        std::mem::take(&mut self.eeprom_dirty)
    }

    fn is_battery_backed(&self) -> bool {
        true
    }

    fn loadram(&mut self, ramdata: &[u8]) -> StrResult<()> {
        let n = ramdata.len().min(self.eeprom.len());
        self.eeprom[..n].copy_from_slice(&ramdata[..n]);
        Ok(())
    }

    fn dumpram(&self) -> Vec<u8> {
        self.eeprom.clone()
    }

    fn set_tilt(&mut self, x: f32, y: f32) {
        self.tilt = (x, y);
    }

    fn take_rom(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.rom)
    }

    fn set_rom(&mut self, rom: Vec<u8>) {
        self.rom = rom;
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn cart() -> MBC7 {
        let mut rom = vec![0u8; 0x8000];
        rom[0x147] = 0x22;
        let mut m = MBC7::new(rom).unwrap();
        m.writerom(0x0000, 0x0A);
        m.writerom(0x4000, 0x40);
        m
    }

    /// Clock one bit into the EEPROM (CS held high).
    fn send_bit(m: &mut MBC7, bit: bool) {
        let di = if bit { 0x02 } else { 0 };
        m.writeram(0xA080, 0x80 | di);
        m.writeram(0xA080, 0xC0 | di);
    }

    fn send(m: &mut MBC7, value: u32, bits: u32) {
        for i in (0..bits).rev() {
            send_bit(m, value >> i & 1 != 0);
        }
    }

    fn deselect(m: &mut MBC7) {
        m.writeram(0xA080, 0x00);
    }

    fn read_word(m: &mut MBC7, addr: u32) -> u16 {
        send(m, 0b1_10 << 8 | addr, 11);
        assert_eq!(m.readram(0xA080) & 1, 0, "dummy zero before data");
        let mut v = 0u16;
        for _ in 0..16 {
            send_bit(m, false);
            v = v << 1 | (m.readram(0xA080) & 1) as u16;
        }
        deselect(m);
        v
    }

    #[test]
    fn eeprom_write_needs_ewen_then_reads_back() {
        let mut m = cart();
        send(&mut m, 0b1_01 << 8 | 5, 11); // WRITE while disabled
        send(&mut m, 0x1234, 16);
        deselect(&mut m);
        assert_eq!(read_word(&mut m, 5), 0xFFFF, "write-protected by default");

        send(&mut m, 0b1_00 << 8 | 0b11 << 6, 11); // EWEN
        deselect(&mut m);
        send(&mut m, 0b1_01 << 8 | 5, 11);
        send(&mut m, 0x1234, 16);
        deselect(&mut m);
        assert_eq!(read_word(&mut m, 5), 0x1234);
        assert!(m.check_and_reset_ram_updated());
        assert_eq!(&m.dumpram()[10..12], &[0x12, 0x34]);
    }

    #[test]
    fn accelerometer_latches_tilt() {
        let mut m = cart();
        m.set_tilt(0.5, -1.0);
        m.writeram(0xA000, 0x55);
        assert_eq!(m.readram(0xA020), 0x00, "reads 0x8000 until latched");
        m.writeram(0xA010, 0xAA);
        let x = u16::from_le_bytes([m.readram(0xA020), m.readram(0xA030)]);
        let y = u16::from_le_bytes([m.readram(0xA040), m.readram(0xA050)]);
        assert_eq!(x, ACCEL_CENTER + 0x38);
        assert_eq!(y, ACCEL_CENTER - 0x70);
    }

    #[test]
    fn registers_need_both_ram_enables() {
        let mut m = cart();
        m.writerom(0x4000, 0x00);
        assert_eq!(m.readram(0xA080), 0xFF);
    }
}
