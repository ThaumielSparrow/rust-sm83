use crate::mbc::MBC;
use crate::StrResult;

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MBC0 {
    #[rkyv(with = rkyv::with::Skip)]
    rom: Vec<u8>,
}

impl MBC0 {
    pub fn new(data: Vec<u8>) -> StrResult<MBC0> {
        Ok(MBC0 { rom: data })
    }
}

impl MBC for MBC0 {
    fn readrom(&self, a: u16) -> u8 {
        // unwrap_or(0xFF) so a rom-less cartridge (post-take_rom, before set_rom)
        // returns open-bus instead of panicking, matching the other MBCs.
        *self.rom.get(a as usize).unwrap_or(&0xFF)
    }
    fn readram(&self, _a: u16) -> u8 {
        0
    }
    fn writerom(&mut self, _a: u16, _v: u8) {
        ()
    }
    fn writeram(&mut self, _a: u16, _v: u8) {
        ()
    }

    fn is_battery_backed(&self) -> bool {
        false
    }
    fn loadram(&mut self, _ramdata: &[u8]) -> StrResult<()> {
        Ok(())
    }
    fn dumpram(&self) -> Vec<u8> {
        Vec::new()
    }
    fn check_and_reset_ram_updated(&mut self) -> bool {
        false
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
