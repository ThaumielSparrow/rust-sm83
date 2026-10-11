use crate::mbc::{ram_banks_with_ram, MBC};
use crate::StrResult;

use std::convert::TryInto;
use std::time;

/// RTC prescaler input: the 32.768 kHz crystal, expressed in emulated
/// T-cycles (dots at 4.194304 MHz) per second.
const TICKS_PER_SECOND: u32 = 4_194_304;

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
    has_rtc: bool,
    /// Live RTC registers: seconds, minutes, hours, day low, day high
    /// (bit 0 = day bit 8, bit 6 = halt, bit 7 = day carry). The clock runs
    /// on emulated time (`tick`); real time that passed while the emulator
    /// was closed is added when the battery save is loaded.
    rtc: [u8; 5],
    rtc_latch: [u8; 5],
    /// Emulated T-cycles since the last whole second.
    rtc_ticks: u32,
    latch_prev: u8,
}

impl MBC3 {
    pub fn new(data: Vec<u8>) -> StrResult<MBC3> {
        let subtype = data[0x147];
        let has_battery = matches!(subtype, 0x0F | 0x10 | 0x13);
        let rambanks = match subtype {
            0x10 | 0x12 | 0x13 => ram_banks_with_ram(data[0x149]),
            _ => 0,
        };
        Ok(MBC3 {
            rom: data,
            ram: vec![0; rambanks * 0x2000],
            rombank: 1,
            rambank: 0,
            rambanks,
            selectrtc: false,
            ram_on: false,
            ram_updated: false,
            has_battery,
            has_rtc: matches!(subtype, 0x0F | 0x10),
            rtc: [0; 5],
            rtc_latch: [0; 5],
            rtc_ticks: 0,
            latch_prev: 0xFF,
        })
    }

    fn halted(&self) -> bool {
        self.rtc[4] & 0x40 != 0
    }

    /// One RTC second, with the hardware's carry chain: each field counts
    /// within its bit width and only carries when it passes its normal limit
    /// (so out-of-range values written by a game wrap like on hardware).
    fn rtc_increment(&mut self) {
        let r = &mut self.rtc;
        r[0] = (r[0] + 1) & 0x3F;
        if r[0] != 60 {
            return;
        }
        r[0] = 0;
        r[1] = (r[1] + 1) & 0x3F;
        if r[1] != 60 {
            return;
        }
        r[1] = 0;
        r[2] = (r[2] + 1) & 0x1F;
        if r[2] != 24 {
            return;
        }
        r[2] = 0;
        let days = ((((r[4] & 1) as u16) << 8) | r[3] as u16) + 1;
        r[3] = days as u8;
        r[4] = (r[4] & 0xFE) | ((days >> 8) & 1) as u8;
        if days > 0x1FF {
            r[3] = 0;
            r[4] = (r[4] & 0xFE) | 0x80; // overflow sets the sticky carry
        }
    }

    fn rtc_seconds(&self) -> u64 {
        let r = &self.rtc;
        let days = (((r[4] & 1) as u64) << 8) | r[3] as u64;
        r[0] as u64 + r[1] as u64 * 60 + r[2] as u64 * 3600 + days * 86400
    }

    fn set_rtc_seconds(&mut self, secs: u64) {
        let days = secs / 86400;
        self.rtc[0] = (secs % 60) as u8;
        self.rtc[1] = ((secs / 60) % 60) as u8;
        self.rtc[2] = ((secs / 3600) % 24) as u8;
        self.rtc[3] = days as u8;
        self.rtc[4] = (self.rtc[4] & 0xFE) | ((days >> 8) & 1) as u8;
        if days > 0x1FF {
            self.rtc[4] |= 0x80;
        }
    }

    /// Advance the clock by real time that passed while not emulating.
    fn advance_wall_clock(&mut self, secs: u64) {
        if self.has_rtc && !self.halted() && secs > 0 {
            let total = self.rtc_seconds() + secs;
            self.set_rtc_seconds(total);
        }
    }

    /// Live clock (registers, latch, sub-second phase), to carry across a
    /// save-state load: the clock lives in the cartridge, not the console.
    pub fn rtc_state(&self) -> ([u8; 5], [u8; 5], u32) {
        (self.rtc, self.rtc_latch, self.rtc_ticks)
    }

    pub fn set_rtc_state(&mut self, state: ([u8; 5], [u8; 5], u32)) {
        (self.rtc, self.rtc_latch, self.rtc_ticks) = state;
    }
}

fn unix_now() -> u64 {
    time::SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
        } else if self.selectrtc && self.has_rtc && self.rambank < 5 {
            self.rtc_latch[self.rambank]
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
                    self.rtc_latch = self.rtc;
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
        } else if self.selectrtc && self.has_rtc && self.rambank < 5 {
            let mask = match self.rambank {
                0 | 1 => 0x3F,
                2 => 0x1F,
                4 => 0xC1,
                _ => 0xFF,
            };
            self.rtc[self.rambank] = v & mask;
            if self.rambank == 0 {
                self.rtc_ticks = 0; // writing seconds resets the prescaler
            }
            self.ram_updated = true;
        }
    }

    fn tick(&mut self, ticks: u32) {
        if !self.has_rtc || self.halted() {
            return;
        }
        self.rtc_ticks += ticks;
        while self.rtc_ticks >= TICKS_PER_SECOND {
            self.rtc_ticks -= TICKS_PER_SECOND;
            self.rtc_increment();
        }
    }

    fn is_battery_backed(&self) -> bool {
        self.has_battery
    }

    /// Accepts three layouts:
    /// - standard (BGB / VBA-M / SameBoy): RAM + 44- or 48-byte RTC footer;
    ///   the clock then catches up by the real time since it was saved
    /// - legacy rust-gbe: 8-byte big-endian wall-clock anchor + RAM
    /// - plain RAM of any size (no RTC data)
    fn loadram(&mut self, ramdata: &[u8]) -> StrResult<()> {
        let ram_len = self.ram.len();
        if ramdata.len() == 8 + ram_len {
            let (anchor, rest) = ramdata.split_at(8);
            self.ram.copy_from_slice(rest);
            if self.has_rtc {
                // The anchor was the unix time at which the clock read zero.
                let anchor = u64::from_be_bytes(anchor.try_into().unwrap());
                self.set_rtc_seconds(unix_now().saturating_sub(anchor));
            }
            return Ok(());
        }

        let n = ramdata.len().min(ram_len);
        self.ram[..n].copy_from_slice(&ramdata[..n]);

        let footer = &ramdata[n..];
        if self.has_rtc && (footer.len() == 44 || footer.len() == 48) {
            for i in 0..5 {
                self.rtc[i] = footer[i * 4];
                self.rtc_latch[i] = footer[(5 + i) * 4];
            }
            let saved_at = if footer.len() == 48 {
                u64::from_le_bytes(footer[40..48].try_into().unwrap())
            } else {
                u32::from_le_bytes(footer[40..44].try_into().unwrap()) as u64
            };
            self.advance_wall_clock(unix_now().saturating_sub(saved_at));
        }
        Ok(())
    }

    /// Writes RAM followed by the standard 48-byte RTC footer (for RTC carts),
    /// so `.gbsave` files are interchangeable with other emulators.
    fn dumpram(&self) -> Vec<u8> {
        let mut file = self.ram.clone();
        if !self.has_rtc {
            return file;
        }
        for r in self.rtc.iter().chain(self.rtc_latch.iter()) {
            file.extend_from_slice(&(*r as u32).to_le_bytes());
        }
        file.extend_from_slice(&unix_now().to_le_bytes());
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

    fn rtc_cart() -> MBC3 {
        let mut rom = vec![0u8; 0x8000];
        rom[0x147] = 0x10; // MBC3+TIMER+RAM+BATTERY
        rom[0x149] = 0x02; // 8 KiB RAM
        MBC3::new(rom).unwrap()
    }

    #[test]
    fn rtc_runs_on_emulated_time_and_latches() {
        let mut m = rtc_cart();
        m.tick(TICKS_PER_SECOND * 61 + 5);
        m.writerom(0x6000, 0);
        m.writerom(0x6000, 1);
        m.writerom(0x0000, 0x0A);
        m.writerom(0x4000, 0x08);
        assert_eq!(m.readram(0xA000), 1, "seconds");
        m.writerom(0x4000, 0x09);
        assert_eq!(m.readram(0xA000), 1, "minutes");
    }

    #[test]
    fn halted_rtc_does_not_count() {
        let mut m = rtc_cart();
        m.writerom(0x0000, 0x0A);
        m.writerom(0x4000, 0x0C);
        m.writeram(0xA000, 0x40); // DH: halt
        m.tick(TICKS_PER_SECOND * 10);
        assert_eq!(m.rtc[0], 0);
    }

    #[test]
    fn day_counter_overflow_sets_carry() {
        let mut m = rtc_cart();
        m.rtc = [59, 59, 23, 0xFF, 0x01];
        m.rtc_increment();
        assert_eq!(m.rtc, [0, 0, 0, 0, 0x80]);
    }

    #[test]
    fn rtc_save_round_trips_through_standard_footer() {
        let mut m = rtc_cart();
        m.ram[0] = 0xAB;
        m.rtc = [7, 5, 3, 2, 0];
        let dump = m.dumpram();
        assert_eq!(dump.len(), 0x2000 + 48);
        let mut m2 = rtc_cart();
        m2.loadram(&dump).unwrap();
        assert_eq!(m2.ram[0], 0xAB);
        // Saved and loaded within the same second or two.
        assert!(m2.rtc_seconds() >= m.rtc_seconds() && m2.rtc_seconds() <= m.rtc_seconds() + 2);
    }

    #[test]
    fn standard_footer_catches_up_by_time_away() {
        let mut m = rtc_cart();
        let mut dump = m.dumpram();
        let saved_at = unix_now() - 3600;
        let n = dump.len();
        dump[n - 8..].copy_from_slice(&saved_at.to_le_bytes());
        m.loadram(&dump).unwrap();
        assert!((3600..=3602).contains(&m.rtc_seconds()));
    }

    #[test]
    fn legacy_prefix_save_still_loads() {
        let anchor = unix_now() - 90;
        let mut data = anchor.to_be_bytes().to_vec();
        data.extend(std::iter::repeat_n(0x5A, 0x2000));
        let mut m = rtc_cart();
        m.loadram(&data).unwrap();
        assert_eq!(m.ram[0], 0x5A);
        assert!((90..=92).contains(&m.rtc_seconds()));
    }

    #[test]
    fn short_plain_save_loads_what_fits() {
        let mut m = rtc_cart();
        m.loadram(&[7u8; 16]).unwrap();
        assert_eq!(&m.ram[..16], &[7u8; 16]);
        assert_eq!(m.ram[16], 0);
    }
}
