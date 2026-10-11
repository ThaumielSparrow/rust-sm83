use crate::mbc::{ram_banks_with_ram, MBC};
use crate::StrResult;

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MBC0 {
    #[rkyv(with = rkyv::with::Skip)]
    rom: Vec<u8>,
    /// Cartridge types 0x08/0x09: up to 8 KiB of RAM, no banking.
    ram: Vec<u8>,
    has_battery: bool,
    ram_updated: bool,
}

impl MBC0 {
    pub fn new(data: Vec<u8>) -> StrResult<MBC0> {
        let (ram_size, has_battery) = match data[0x147] {
            0x08 => (0x2000, false),
            0x09 => (0x2000, true),
            _ => (0, false),
        };
        // The header may list more, but without a mapper only 8 KiB is visible.
        let ram_size = if ram_size > 0 { (ram_banks_with_ram(data[0x149]) * 0x2000).min(ram_size) } else { 0 };
        Ok(MBC0 { rom: data, ram: vec![0; ram_size], has_battery, ram_updated: false })
    }
}

impl MBC for MBC0 {
    fn readrom(&self, a: u16) -> u8 {
        // unwrap_or(0xFF) so a rom-less cartridge (post-take_rom, before set_rom)
        // returns open-bus instead of panicking, matching the other MBCs.
        *self.rom.get(a as usize).unwrap_or(&0xFF)
    }
    fn readram(&self, a: u16) -> u8 {
        *self.ram.get((a & 0x1FFF) as usize).unwrap_or(&0xFF)
    }
    fn writerom(&mut self, _a: u16, _v: u8) {}
    fn writeram(&mut self, a: u16, v: u8) {
        if let Some(b) = self.ram.get_mut((a & 0x1FFF) as usize) {
            *b = v;
            self.ram_updated = true;
        }
    }

    fn is_battery_backed(&self) -> bool {
        self.has_battery
    }
    fn loadram(&mut self, ramdata: &[u8]) -> StrResult<()> {
        let n = ramdata.len().min(self.ram.len());
        self.ram[..n].copy_from_slice(&ramdata[..n]);
        Ok(())
    }
    fn dumpram(&self) -> Vec<u8> {
        self.ram.clone()
    }
    fn check_and_reset_ram_updated(&mut self) -> bool {
        std::mem::take(&mut self.ram_updated)
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
    #[test]
    fn rom_plus_battery_ram_cart_stores_ram() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x147] = 0x09; // ROM+RAM+BATTERY
        rom[0x149] = 0x02;
        let mut m = MBC0::new(rom).unwrap();
        m.writeram(0xA123, 0x77);
        assert_eq!(m.readram(0xA123), 0x77);
        assert!(m.is_battery_backed());
        assert!(m.check_and_reset_ram_updated());
        assert_eq!(m.dumpram().len(), 0x2000);
    }

    use super::*;

    fn rom() -> Vec<u8> {
        let mut data = vec![0u8; 0x8000];
        data[0x147] = 0x00; // MBC0
        data
    }

    #[test]
    fn take_and_set_rom_round_trip() {
        let mut mbc = MBC0::new(rom()).unwrap();
        mbc.set_rom({
            let mut r = rom();
            r[0x0100] = 0xAB;
            r
        });
        let taken = mbc.take_rom();
        assert!(!taken.is_empty(), "take_rom yields the rom bytes");
        assert_eq!(mbc.readrom(0x0100), 0xFF, "rom-less read returns 0xFF, no panic");
        mbc.set_rom(taken);
        assert_eq!(mbc.readrom(0x0100), 0xAB, "rom restored after set_rom");
    }
}
