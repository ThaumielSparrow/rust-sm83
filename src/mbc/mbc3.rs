use crate::mbc::{ram_banks_with_ram, MBC};
use crate::StrResult;

use std::convert::TryInto;
use std::time;

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MBC3 {
    #[rkyv(with = rkyv::with::Skip)]
    rom: Vec<u8>,
    ram: Vec<u8>,
    rombank: usize,
    rambank: usize,
    rambanks: usize,
    selectrtc: bool,
    ram_on: bool,
    ram_updated: bool,
    has_battery: bool,
    pub(crate) rtc_ram: [u8; 5],
    rtc_ram_latch: [u8; 5],
    pub(crate) rtc_zero: Option<u64>,
    latch_prev: u8,
}

impl MBC3 {
    pub fn new(data: Vec<u8>) -> StrResult<MBC3> {
        let subtype = data[0x147];
        let has_battery = match subtype {
            0x0F | 0x10 | 0x13 => true,
            _ => false,
        };
        let rambanks = match subtype {
            0x10 | 0x12 | 0x13 => ram_banks_with_ram(data[0x149]),
            _ => 0,
        };
        let ramsize = rambanks * 0x2000;
        let rtc = match subtype {
            0x0F | 0x10 => Some(0),
            _ => None,
        };

        let res = MBC3 {
            rom: data,
            ram: ::std::iter::repeat(0u8).take(ramsize).collect(),
            rombank: 1,
            rambank: 0,
            rambanks: rambanks,
            selectrtc: false,
            ram_on: false,
            ram_updated: false,
            has_battery: has_battery,
            rtc_ram: [0u8; 5],
            rtc_ram_latch: [0u8; 5],
            rtc_zero: rtc,
            latch_prev: 0xFF,
        };

        Ok(res)
    }

    fn latch_rtc_reg(&mut self) {
        self.calc_rtc_reg();
        self.rtc_ram_latch.clone_from_slice(&self.rtc_ram);
    }

    fn calc_rtc_reg(&mut self) {
        // Do not modify regs when halted
        if self.rtc_ram[4] & 0x40 == 0x40 {
            return;
        }

        let tzero = match self.rtc_zero {
            Some(t) => match time::UNIX_EPOCH.checked_add(time::Duration::from_secs(t)) {
                Some(st) => st,
                None => return,
            },
            None => return,
        };

        if self.compute_difftime() == self.rtc_zero {
            // No time has passed. Do not alter registers
            return;
        }

        let difftime = match time::SystemTime::now().duration_since(tzero) {
            Ok(n) => n.as_secs(),
            _ => 0,
        };
        self.rtc_ram[0] = (difftime % 60) as u8;
        self.rtc_ram[1] = ((difftime / 60) % 60) as u8;
        self.rtc_ram[2] = ((difftime / 3600) % 24) as u8;
        let days = difftime / (3600 * 24);
        self.rtc_ram[3] = days as u8;
        self.rtc_ram[4] = (self.rtc_ram[4] & 0xFE) | (((days >> 8) & 0x01) as u8);
        if days >= 512 {
            self.rtc_ram[4] |= 0x80;
            self.calc_rtc_zero();
        }
    }

    fn compute_difftime(&self) -> Option<u64> {
        if self.rtc_zero.is_none() {
            return None;
        }
        let mut difftime = time::SystemTime::now()
            .duration_since(time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        difftime = difftime.saturating_sub(self.rtc_ram[0] as u64);
        difftime = difftime.saturating_sub((self.rtc_ram[1] as u64) * 60);
        difftime = difftime.saturating_sub((self.rtc_ram[2] as u64) * 3600);
        let days = ((self.rtc_ram[4] as u64 & 0x1) << 8) | (self.rtc_ram[3] as u64);
        difftime = difftime.saturating_sub(days * 3600 * 24);
        Some(difftime)
    }

    fn calc_rtc_zero(&mut self) {
        self.rtc_zero = self.compute_difftime();
    }
}

impl MBC for MBC3 {
    fn readrom(&self, a: u16) -> u8 {
        let idx = if a < 0x4000 {
            a as usize
        } else {
            self.rombank * 0x4000 | ((a as usize) & 0x3FFF)
        };
        *self.rom.get(idx).unwrap_or(&0xFF)
    }
    fn readram(&self, a: u16) -> u8 {
        if !self.ram_on {
            return 0xFF;
        }
        if !self.selectrtc && self.rambank < self.rambanks {
            self.ram[self.rambank * 0x2000 | ((a as usize) & 0x1FFF)]
        } else if self.selectrtc && self.rambank < 5 {
            self.rtc_ram_latch[self.rambank]
        } else {
            0xFF
        }
    }
    fn writerom(&mut self, a: u16, v: u8) {
        match a {
            0x0000..=0x1FFF => self.ram_on = (v & 0x0F) == 0x0A,
            0x2000..=0x3FFF => {
                self.rombank = match v & 0x7F {
                    0 => 1,
                    n => n as usize,
                }
            }
            0x4000..=0x5FFF => {
                self.selectrtc = v & 0x8 == 0x8;
                self.rambank = (v & 0x7) as usize;
            }
            0x6000..=0x7FFF => {
                if self.latch_prev == 0x00 && v == 0x01 {
                    self.latch_rtc_reg();
                }
                self.latch_prev = v;
            }
            _ => (),
        }
    }
    fn writeram(&mut self, a: u16, v: u8) {
        if !self.ram_on {
            return;
        }
        if !self.selectrtc && self.rambank < self.rambanks {
            self.ram[self.rambank * 0x2000 | ((a as usize) & 0x1FFF)] = v;
            self.ram_updated = true;
        } else if self.selectrtc && self.rambank < 5 {
            self.calc_rtc_reg();
            let vmask = match self.rambank {
                0 | 1 => 0x3F,
                2 => 0x1F,
                4 => 0xC1,
                _ => 0xFF,
            };
            self.rtc_ram[self.rambank] = v & vmask;
            self.calc_rtc_zero();
            self.ram_updated = true;
        }
    }

    fn is_battery_backed(&self) -> bool {
        self.has_battery
    }

    /// Accepts three layouts:
    /// - legacy rust-gbe: 8-byte big-endian `rtc_zero` prefix + RAM
    /// - standard (BGB / VBA-M / SameBoy): RAM + 44- or 48-byte RTC footer
    /// - plain RAM of any size (no RTC data)
    fn loadram(&mut self, ramdata: &[u8]) -> StrResult<()> {
        let ram_len = self.ram.len();
        if ramdata.len() == 8 + ram_len {
            let (int_bytes, rest) = ramdata.split_at(8);
            let rtc = u64::from_be_bytes(int_bytes.try_into().unwrap());
            if self.rtc_zero.is_some() {
                self.rtc_zero = Some(rtc);
            }
            self.ram.copy_from_slice(rest);
            return Ok(());
        }

        let n = ramdata.len().min(ram_len);
        self.ram[..n].copy_from_slice(&ramdata[..n]);

        let footer = &ramdata[n..];
        if self.rtc_zero.is_some() && (footer.len() == 44 || footer.len() == 48) {
            let reg = |i: usize| footer[i * 4];
            let saved_at = if footer.len() == 48 {
                u64::from_le_bytes(footer[40..48].try_into().unwrap())
            } else {
                u32::from_le_bytes(footer[40..44].try_into().unwrap()) as u64
            };
            for i in 0..5 {
                self.rtc_ram[i] = reg(i);
                self.rtc_ram_latch[i] = reg(5 + i);
            }
            let days = (((self.rtc_ram[4] & 0x01) as u64) << 8) | self.rtc_ram[3] as u64;
            let elapsed = self.rtc_ram[0] as u64
                + self.rtc_ram[1] as u64 * 60
                + self.rtc_ram[2] as u64 * 3600
                + days * 86400;
            self.rtc_zero = Some(saved_at.saturating_sub(elapsed));
        }
        Ok(())
    }

    /// Writes RAM followed by the standard 48-byte RTC footer (for RTC carts),
    /// so `.gbsave` files are interchangeable with other emulators.
    fn dumpram(&self) -> Vec<u8> {
        let mut file = self.ram.clone();
        if self.rtc_zero.is_none() {
            return file;
        }
        let now = time::SystemTime::now()
            .duration_since(time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut regs = self.rtc_ram;
        if regs[4] & 0x40 == 0 {
            // Running clock: derive the live registers from rtc_zero.
            let diff = now.saturating_sub(self.rtc_zero.unwrap_or(now));
            let days = diff / 86400;
            regs[0] = (diff % 60) as u8;
            regs[1] = ((diff / 60) % 60) as u8;
            regs[2] = ((diff / 3600) % 24) as u8;
            regs[3] = days as u8;
            regs[4] = (regs[4] & 0xFE) | ((days >> 8) & 0x01) as u8;
            if days >= 512 {
                regs[4] |= 0x80;
            }
        }
        for r in regs.iter().chain(self.rtc_ram_latch.iter()) {
            file.extend_from_slice(&(*r as u32).to_le_bytes());
        }
        file.extend_from_slice(&now.to_le_bytes());
        file
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

    #[test]
    fn rtc_compute_difftime_does_not_underflow_on_hostile_save() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x147] = 0x10; // MBC3+RAM+Battery+Timer
        rom[0x148] = 0x00;
        rom[0x149] = 0x02; // 8 KiB RAM
        let mut mbc = MBC3::new(rom).unwrap();

        // Hostile encoded RTC: large values.
        mbc.rtc_ram = [59, 59, 23, 0xFF, 0x01];
        // Force rtc_zero into the future.
        mbc.rtc_zero = Some(u64::MAX - 1000);

        // Must not panic.
        mbc.calc_rtc_reg();
    }

    fn rtc_cart() -> MBC3 {
        let mut rom = vec![0u8; 0x8000];
        rom[0x147] = 0x10; // MBC3+TIMER+RAM+BATTERY
        rom[0x149] = 0x02; // 8 KiB RAM
        MBC3::new(rom).unwrap()
    }

    #[test]
    fn rtc_save_round_trips_through_standard_footer() {
        let mut m = rtc_cart();
        m.ram[0] = 0xAB;
        let now = time::SystemTime::now()
            .duration_since(time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let zero = now - (3 * 86400 + 5 * 3600 + 7);
        m.rtc_zero = Some(zero);
        let dump = m.dumpram();
        assert_eq!(dump.len(), 0x2000 + 48);
        let mut m2 = rtc_cart();
        m2.loadram(&dump).unwrap();
        assert_eq!(m2.ram[0], 0xAB);
        assert_eq!(m2.rtc_zero, Some(zero));
    }

    #[test]
    fn legacy_prefix_save_still_loads() {
        let mut data = 1234u64.to_be_bytes().to_vec();
        data.extend(std::iter::repeat_n(0x5A, 0x2000));
        let mut m = rtc_cart();
        m.loadram(&data).unwrap();
        assert_eq!(m.rtc_zero, Some(1234));
        assert_eq!(m.ram[0], 0x5A);
    }

    #[test]
    fn short_plain_save_loads_what_fits() {
        let mut m = rtc_cart();
        m.loadram(&[7u8; 16]).unwrap();
        assert_eq!(&m.ram[..16], &[7u8; 16]);
        assert_eq!(m.ram[16], 0);
    }
}
