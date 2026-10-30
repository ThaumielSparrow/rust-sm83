use crate::mbc::{ram_banks_with_ram, rom_banks, MBC};
use crate::StrResult;

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MBC1 {
    #[rkyv(with = rkyv::with::Skip)]
    rom: Vec<u8>,
    ram: Vec<u8>,
    ram_on: bool,
    ram_updated: bool,
    banking_mode: u8,
    /// BANK1 register (2000-3FFF), 5 bits, never 0.
    bank1: usize,
    /// BANK2 register (4000-5FFF), 2 bits: upper ROM bits or RAM bank.
    bank2: usize,
    has_battery: bool,
    rombanks: usize,
    rambanks: usize,
    /// MBC1M multicart wiring: BANK2 sits above 4 BANK1 bits, not 5.
    multicart: bool,
}

impl MBC1 {
    pub fn new(data: Vec<u8>) -> StrResult<MBC1> {
        let (has_battery, rambanks) = match data[0x147] {
            0x02 => (false, ram_banks_with_ram(data[0x149])),
            0x03 => (true, ram_banks_with_ram(data[0x149])),
            _ => (false, 0),
        };
        let rombanks = rom_banks(data[0x148]);
        // rombank writes compute `% rombanks`; a size byte > 8 yields 0 banks
        // and would panic there. MBC0/MBC3 tolerate any size byte, so this is
        // rejected per-mapper rather than in get_mbc.
        if rombanks == 0 {
            return Err("Invalid ROM size in cartridge header");
        }
        let ramsize = rambanks * 0x2000;
        let multicart = is_multicart(&data);

        Ok(MBC1 {
            rom: data,
            ram: vec![0; ramsize],
            ram_on: false,
            banking_mode: 0,
            bank1: 1,
            bank2: 0,
            ram_updated: false,
            has_battery,
            rombanks,
            rambanks,
            multicart,
        })
    }

    fn bank1_bits(&self) -> u32 {
        if self.multicart { 4 } else { 5 }
    }

    fn rom_bank_low(&self) -> usize {
        if self.banking_mode == 1 {
            (self.bank2 << self.bank1_bits()) % self.rombanks
        } else {
            0
        }
    }

    fn rom_bank_high(&self) -> usize {
        let mask = (1 << self.bank1_bits()) - 1;
        ((self.bank2 << self.bank1_bits()) | (self.bank1 & mask)) % self.rombanks
    }

    fn ram_offset(&self, a: u16) -> usize {
        let bank = if self.banking_mode == 1 && self.rambanks > 1 {
            self.bank2 % self.rambanks
        } else {
            0
        };
        (bank * 0x2000) | ((a & 0x1FFF) as usize)
    }
}

/// MBC1M multicarts (8 Mbit compilations) are wired so BANK2 selects 256 KiB
/// games; each game has its own header. Detect them by a second Nintendo logo
/// at the start of bank 0x10, as other emulators do.
fn is_multicart(rom: &[u8]) -> bool {
    const LOGO: std::ops::Range<usize> = 0x104..0x134;
    rom.len() == 0x100000
        && rom[0x40000 + LOGO.start..0x40000 + LOGO.end] == rom[LOGO]
}

impl MBC for MBC1 {
    fn readrom(&self, a: u16) -> u8 {
        let bank = if a < 0x4000 { self.rom_bank_low() } else { self.rom_bank_high() };
        let idx = (bank * 0x4000) | ((a as usize) & 0x3FFF);
        *self.rom.get(idx).unwrap_or(&0xFF)
    }
    fn readram(&self, a: u16) -> u8 {
        if !self.ram_on || self.ram.is_empty() {
            return 0xFF;
        }
        *self.ram.get(self.ram_offset(a)).unwrap_or(&0xFF)
    }

    fn writerom(&mut self, a: u16, v: u8) {
        match a {
            0x0000..=0x1FFF => self.ram_on = v & 0xF == 0xA,
            // The zero check sees all 5 bits, even on multicarts.
            0x2000..=0x3FFF => self.bank1 = ((v as usize) & 0x1F).max(1),
            0x4000..=0x5FFF => self.bank2 = (v as usize) & 0x03,
            0x6000..=0x7FFF => self.banking_mode = v & 0x01,
            _ => (),
        }
    }

    fn writeram(&mut self, a: u16, v: u8) {
        if !self.ram_on || self.ram.is_empty() {
            return;
        }
        let address = self.ram_offset(a);
        if address < self.ram.len() {
            self.ram[address] = v;
            self.ram_updated = true;
        }
    }

    fn is_battery_backed(&self) -> bool {
        self.has_battery
    }

    fn loadram(&mut self, ramdata: &[u8]) -> StrResult<()> {
        // Accept saves of any size (other emulators pad or append footers):
        // load whatever overlaps and leave the rest zeroed.
        let n = ramdata.len().min(self.ram.len());
        self.ram[..n].copy_from_slice(&ramdata[..n]);

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
    use crate::mbc::MBC;

    fn no_ram_cart() -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x147] = 0x01; // MBC1, no RAM
        rom[0x148] = 0x00;
        rom[0x149] = 0x00;
        rom
    }

    #[test]
    fn mbc1_no_ram_read_returns_0xff_no_panic() {
        let mut mbc = MBC1::new(no_ram_cart()).unwrap();
        mbc.writerom(0x0000, 0x0A);
        assert_eq!(mbc.readram(0xA000), 0xFF);
    }

    #[test]
    fn mbc1_no_ram_write_does_not_panic() {
        let mut mbc = MBC1::new(no_ram_cart()).unwrap();
        mbc.writerom(0x0000, 0x0A);
        mbc.writeram(0xA000, 0x42);
    }

    #[test]
    fn take_and_set_rom_round_trip() {
        let mut mbc = MBC1::new(no_ram_cart()).unwrap();
        let header_byte = mbc.readrom(0x0147);
        assert_eq!(header_byte, 0x01);
        let rom = mbc.take_rom();
        assert!(!rom.is_empty(), "take_rom yields the rom bytes");
        assert_eq!(mbc.readrom(0x0000), 0xFF, "rom-less read returns 0xFF");
        mbc.set_rom(rom);
        assert_eq!(mbc.readrom(0x0147), 0x01, "rom restored after set_rom");
    }
}
