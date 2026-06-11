use crate::mbc::{ram_banks, rom_banks, MBC};
use crate::StrResult;

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MBC5 {
    rom: Vec<u8>,
    ram: Vec<u8>,
    rombank: usize,
    rambank: usize,
    ram_on: bool,
    ram_updated: bool,
    has_battery: bool,
    rombanks: usize,
    rambanks: usize,
    has_rumble: bool,
}

impl MBC5 {
    pub fn new(data: Vec<u8>) -> StrResult<MBC5> {
        let (has_battery, has_rumble, rambanks) = match data[0x147] {
            0x19 => (false, false, 0),
            0x1A => (false, false, ram_banks(data[0x149])),
            0x1B => (true,  false, ram_banks(data[0x149])),
            0x1C => (false, true,  0),
            0x1D => (false, true,  ram_banks(data[0x149])),
            0x1E => (true,  true,  ram_banks(data[0x149])),
            _    => (false, false, 0),
        };
        let ramsize = 0x2000 * rambanks;
        let rombanks = rom_banks(data[0x148]);

        let res = MBC5 {
            rom: data,
            ram: ::std::iter::repeat(0u8).take(ramsize).collect(),
            rombank: 1,
            rambank: 0,
            ram_updated: false,
            ram_on: false,
            has_battery: has_battery,
            rombanks: rombanks,
            rambanks: rambanks,
            has_rumble: has_rumble,
        };

        Ok(res)
    }
}

impl MBC for MBC5 {
    fn readrom(&self, a: u16) -> u8 {
        let idx = if a < 0x4000 {
            a as usize
        } else {
            self.rombank * 0x4000 | ((a as usize) & 0x3FFF)
        };
        *self.rom.get(idx).unwrap_or(&0)
    }
    fn readram(&self, a: u16) -> u8 {
        if !self.ram_on || self.ram.is_empty() {
            return 0xFF;
        }
        let idx = self.rambank * 0x2000 | ((a as usize) & 0x1FFF);
        *self.ram.get(idx).unwrap_or(&0xFF)
    }
    fn writerom(&mut self, a: u16, v: u8) {
        match a {
            0x0000..=0x1FFF => self.ram_on = v & 0x0F == 0x0A,
            0x2000..=0x2FFF => {
                self.rombank = ((self.rombank & 0x100) | (v as usize)) % self.rombanks
            }
            0x3000..=0x3FFF => {
                self.rombank =
                    ((self.rombank & 0x0FF) | (((v & 0x1) as usize) << 8)) % self.rombanks
            }
            0x4000..=0x5FFF => {
                if self.rambanks == 0 {
                    return; // no RAM; rumble bit, if any, has no host effect
                }
                let bank_mask: usize = if self.has_rumble { 0x07 } else { 0x0F };
                if self.rambanks > 1 {
                    self.rambank = (v as usize) & bank_mask;
                }
                // Bit 3 on rumble carts drives the motor; we don't implement physical rumble,
                // but it must NOT be routed into rambank.
            }
            0x6000..=0x7FFF => { /* ? */ }
            _ => (),
        }
    }
    fn writeram(&mut self, a: u16, v: u8) {
        if !self.ram_on || self.ram.is_empty() {
            return;
        }
        let idx = self.rambank * 0x2000 | ((a as usize) & 0x1FFF);
        if idx < self.ram.len() {
            self.ram[idx] = v;
            self.ram_updated = true;
        }
    }

    fn is_battery_backed(&self) -> bool {
        self.has_battery
    }

    fn loadram(&mut self, ramdata: &[u8]) -> StrResult<()> {
        if ramdata.len() != self.ram.len() {
            return Err("Loaded RAM has incorrect length");
        }

        self.ram = ramdata.to_vec();

        Ok(())
    }

    fn dumpram(&self) -> Vec<u8> {
        self.ram.to_vec()
    }

    fn check_and_reset_ram_updated(&mut self) -> bool {
        let result = self.ram_updated;
        self.ram_updated = false;
        result
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::mbc::MBC;

    fn rumble_cart_4_banks() -> Vec<u8> {
        let mut rom = vec![0u8; 0x40000];
        rom[0x147] = 0x1D; // MBC5 + rumble + RAM, no battery
        rom[0x148] = 0x00; // 32 KiB ROM
        // 32 KiB RAM (4 banks)
        rom[0x149] = 0x03;
        rom
    }

    #[test]
    fn mbc5_rumble_bit_does_not_affect_rambank() {
        let mut mbc = MBC5::new(rumble_cart_4_banks()).unwrap();
        // Enable RAM
        mbc.writerom(0x0000, 0x0A);
        // Select bank 2 with rumble bit set: v = 0x0A (bit3=1, bits0-2=010=2)
        mbc.writerom(0x4000, 0x0A);
        // Write a marker into bank 2
        mbc.writeram(0xA000, 0x42);
        // Toggle rumble off (v = 0x02)
        mbc.writerom(0x4000, 0x02);
        // Should still be reading bank 2, so the marker is visible
        assert_eq!(mbc.readram(0xA000), 0x42,
            "rumble bit must not switch RAM banks");
    }

    fn no_ram_mbc5_cart() -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x147] = 0x19; // MBC5, no RAM
        rom[0x148] = 0x00;
        rom[0x149] = 0x00;
        rom
    }

    #[test]
    fn mbc5_no_ram_read_returns_0xff_no_panic() {
        let mut mbc = MBC5::new(no_ram_mbc5_cart()).unwrap();
        mbc.writerom(0x0000, 0x0A);
        assert_eq!(mbc.readram(0xA000), 0xFF);
    }

    #[test]
    fn mbc5_no_ram_bank_write_does_not_panic() {
        let mut mbc = MBC5::new(no_ram_mbc5_cart()).unwrap();
        mbc.writerom(0x4000, 0x02);
    }
}
