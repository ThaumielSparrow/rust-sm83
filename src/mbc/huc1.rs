//! Hudson HuC1: MBC1-like ROM/RAM banking plus an infrared port, which is
//! emulated as permanently dark (no other Game Boy in range).
use crate::mbc::{ram_banks_with_ram, rom_banks, MBC};
use crate::StrResult;

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct HuC1 {
    #[rkyv(with = rkyv::with::Skip)]
    rom: Vec<u8>,
    ram: Vec<u8>,
    rombank: usize,
    rambank: usize,
    rombanks: usize,
    rambanks: usize,
    /// 0000-1FFF = 0x0E maps the IR port at A000-BFFF instead of RAM.
    ir_mode: bool,
    ram_updated: bool,
}

impl HuC1 {
    pub fn new(data: Vec<u8>) -> StrResult<HuC1> {
        let rombanks = rom_banks(data[0x148]);
        if rombanks == 0 {
            return Err("Invalid ROM size in cartridge header");
        }
        let rambanks = ram_banks_with_ram(data[0x149]);
        Ok(HuC1 {
            rom: data,
            ram: vec![0; rambanks * 0x2000],
            rombank: 1,
            rambank: 0,
            rombanks,
            rambanks,
            ir_mode: false,
            ram_updated: false,
        })
    }
}

impl MBC for HuC1 {
    fn readrom(&self, a: u16) -> u8 {
        let bank = if a < 0x4000 { 0 } else { self.rombank };
        *self.rom.get((bank * 0x4000) | (a as usize & 0x3FFF)).unwrap_or(&0xFF)
    }

    fn readram(&self, a: u16) -> u8 {
        if self.ir_mode {
            return 0xC0; // IR receiver: no light
        }
        *self.ram.get((self.rambank * 0x2000) | (a as usize & 0x1FFF)).unwrap_or(&0xFF)
    }

    fn writerom(&mut self, a: u16, v: u8) {
        match a {
            0x0000..=0x1FFF => self.ir_mode = v & 0x0F == 0x0E,
            0x2000..=0x3FFF => self.rombank = ((v as usize) & 0x3F).max(1) % self.rombanks,
            0x4000..=0x5FFF => self.rambank = (v as usize & 0x03) % self.rambanks.max(1),
            _ => {}
        }
    }

    fn writeram(&mut self, a: u16, v: u8) {
        if self.ir_mode {
            return; // IR LED: nothing to drive
        }
        let i = (self.rambank * 0x2000) | (a as usize & 0x1FFF);
        if let Some(b) = self.ram.get_mut(i) {
            *b = v;
            self.ram_updated = true;
        }
    }

    fn check_and_reset_ram_updated(&mut self) -> bool {
        std::mem::take(&mut self.ram_updated)
    }

    fn is_battery_backed(&self) -> bool {
        true // HuC1 is only made as 0xFF, HuC1+RAM+BATTERY
    }

    fn loadram(&mut self, ramdata: &[u8]) -> StrResult<()> {
        let n = ramdata.len().min(self.ram.len());
        self.ram[..n].copy_from_slice(&ramdata[..n]);
        Ok(())
    }

    fn dumpram(&self) -> Vec<u8> {
        self.ram.clone()
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

    fn cart() -> Vec<u8> {
        let mut rom = vec![0u8; 0x10000]; // 4 banks
        rom[0x147] = 0xFF;
        rom[0x148] = 0x01;
        rom[0x149] = 0x02;
        for bank in 0..4 {
            rom[bank * 0x4000 + 0x10] = bank as u8;
        }
        rom
    }

    #[test]
    fn banks_rom_and_ram_and_reads_ir_as_dark() {
        let mut m = HuC1::new(cart()).unwrap();
        m.writerom(0x2000, 3);
        assert_eq!(m.readrom(0x4010), 3);
        m.writerom(0x0000, 0x0A);
        m.writeram(0xA000, 0x5A);
        assert_eq!(m.readram(0xA000), 0x5A);
        m.writerom(0x0000, 0x0E);
        assert_eq!(m.readram(0xA000), 0xC0, "IR mode reads the receiver");
        m.writeram(0xA000, 0x00);
        m.writerom(0x0000, 0x0A);
        assert_eq!(m.readram(0xA000), 0x5A, "IR writes don't touch RAM");
    }
}
