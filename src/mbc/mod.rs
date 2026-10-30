use crate::StrResult;
use std::fs::{self, File};
use std::io;
use std::io::prelude::*;
use std::path;

mod mbc0;
mod mbc1;
mod mbc2;
mod mbc3;
mod mbc5;
mod huc1;
mod mbc7;

pub trait MBC: Send {
    fn readrom(&self, a: u16) -> u8;
    fn readram(&self, a: u16) -> u8;
    fn writerom(&mut self, a: u16, v: u8);
    fn writeram(&mut self, a: u16, v: u8);
    fn check_and_reset_ram_updated(&mut self) -> bool;

    fn is_battery_backed(&self) -> bool;
    fn loadram(&mut self, ramdata: &[u8]) -> StrResult<()>;
    fn dumpram(&self) -> Vec<u8>;

    /// Move the ROM bytes out, leaving the cartridge rom-less (reads return 0xFF).
    /// Used to keep the immutable ROM out of rewind/save-state payloads.
    fn take_rom(&mut self) -> Vec<u8>;
    /// Move ROM bytes back in after a rom-less restore.
    fn set_rom(&mut self, rom: Vec<u8>);

    /// Advance cartridge hardware clocked by the system (the MBC3 RTC) by
    /// `ticks` real-time T-cycles (4.194304 MHz).
    fn tick(&mut self, _ticks: u32) {}

    /// MBC7 accelerometer input from the host, in g per axis (x right,
    /// y down when tilting the top away).
    fn set_tilt(&mut self, _x: f32, _y: f32) {}

    /// MBC5 rumble carts: whether the motor is currently driven.
    fn rumble(&self) -> bool {
        false
    }

    fn get_save_path(&self) -> Option<String> {
        None // Default implementation for non-file-backed MBCs
    }

    fn romname(&self) -> String {
        const TITLE_START: u16 = 0x134;
        const CGB_FLAG: u16 = 0x143;

        let title_size = match self.readrom(CGB_FLAG) & 0x80 {
            0x80 => 11,
            _ => 16,
        };

        let mut result = String::with_capacity(title_size as usize);

        for i in 0..title_size {
            match self.readrom(TITLE_START + i) {
                0 => break,
                v => result.push(v as char),
            }
        }

        result
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum MbcState {
    Mbc0(mbc0::MBC0),
    Mbc1(mbc1::MBC1),
    Mbc2(mbc2::MBC2),
    Mbc3(mbc3::MBC3),
    Mbc5(mbc5::MBC5),
    HuC1(huc1::HuC1),
    Mbc7(mbc7::MBC7),
}

/// Forward a call to whichever mapper the cartridge has.
macro_rules! dispatch {
    ($self:ident, $m:ident => $call:expr) => {
        match $self {
            MbcState::Mbc0($m) => $call,
            MbcState::Mbc1($m) => $call,
            MbcState::Mbc2($m) => $call,
            MbcState::Mbc3($m) => $call,
            MbcState::Mbc5($m) => $call,
            MbcState::HuC1($m) => $call,
            MbcState::Mbc7($m) => $call,
        }
    };
}

impl MBC for MbcState {
    fn readrom(&self, a: u16) -> u8 {
        dispatch!(self, m => m.readrom(a))
    }
    fn readram(&self, a: u16) -> u8 {
        dispatch!(self, m => m.readram(a))
    }
    fn writerom(&mut self, a: u16, v: u8) {
        dispatch!(self, m => m.writerom(a, v))
    }
    fn writeram(&mut self, a: u16, v: u8) {
        dispatch!(self, m => m.writeram(a, v))
    }
    fn check_and_reset_ram_updated(&mut self) -> bool {
        dispatch!(self, m => m.check_and_reset_ram_updated())
    }
    fn is_battery_backed(&self) -> bool {
        dispatch!(self, m => m.is_battery_backed())
    }
    fn loadram(&mut self, ramdata: &[u8]) -> StrResult<()> {
        dispatch!(self, m => m.loadram(ramdata))
    }
    fn dumpram(&self) -> Vec<u8> {
        dispatch!(self, m => m.dumpram())
    }
    fn take_rom(&mut self) -> Vec<u8> {
        dispatch!(self, m => m.take_rom())
    }
    fn set_rom(&mut self, rom: Vec<u8>) {
        dispatch!(self, m => m.set_rom(rom))
    }
    fn rumble(&self) -> bool {
        dispatch!(self, m => m.rumble())
    }
    fn tick(&mut self, ticks: u32) {
        dispatch!(self, m => m.tick(ticks))
    }
    fn set_tilt(&mut self, x: f32, y: f32) {
        dispatch!(self, m => m.set_tilt(x, y))
    }
}

pub fn get_mbc(data: Vec<u8>, skip_checksum: bool) -> StrResult<MbcState> {
    if data.len() < 0x150 {
        return Err("ROM size too small");
    }
    if !skip_checksum {
        check_checksum(&data)?;
    }
    match data[0x147] {
        0x00 | 0x08 | 0x09 => mbc0::MBC0::new(data).map(MbcState::Mbc0),
        0x01..=0x03 => mbc1::MBC1::new(data).map(MbcState::Mbc1),
        0x05..=0x06 => mbc2::MBC2::new(data).map(MbcState::Mbc2),
        0x0F..=0x13 => mbc3::MBC3::new(data).map(MbcState::Mbc3),
        0x19..=0x1E => mbc5::MBC5::new(data).map(MbcState::Mbc5),
        0x22 => mbc7::MBC7::new(data).map(MbcState::Mbc7),
        0xFF => huc1::HuC1::new(data).map(MbcState::HuC1),
        _ => Err("Unsupported MBC type"),
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct FileBackedMBC {
    rampath: String,
    mbc: MbcState,
}

impl FileBackedMBC {
    pub fn new(rompath: path::PathBuf, skip_checksum: bool) -> StrResult<FileBackedMBC> {
        let mut data = vec![];
        File::open(&rompath)
            .and_then(|mut f| f.read_to_end(&mut data))
            .map_err(|_| "Could not read ROM")?;
        Self::from_bytes(data, &rompath, skip_checksum)
    }

    /// ROM already in memory (e.g. unpacked from an archive); battery saves
    /// go next to `save_base` as `<save_base>.gbsave`.
    pub fn from_bytes(
        data: Vec<u8>,
        save_base: &path::Path,
        skip_checksum: bool,
    ) -> StrResult<FileBackedMBC> {
        let mut mbc = get_mbc(data, skip_checksum)?;

        let rampath = save_base.with_extension("gbsave");
        // println!("DEBUG: FileBackedMBC will use save path: {}", rampath.display());

        if mbc.is_battery_backed() {
            match fs::File::open(&rampath) {
                Ok(mut file) => {
                    let mut ramdata: Vec<u8> = vec![];
                    match file.read_to_end(&mut ramdata) {
                        Err(..) => return Err("Error while reading existing save file"),
                        Ok(..) => {
                            // println!("DEBUG: Loaded existing save file with {} bytes", ramdata.len());
                            mbc.loadram(&ramdata)?;
                        }
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::NotFound => {
                    // println!("DEBUG: No existing save file found, starting fresh");
                }
                Err(_) => return Err("Error loading existing save file"),
            }
        }

        Ok(FileBackedMBC {
            rampath: rampath.to_string_lossy().to_string(),
            mbc,
        })
    }
}

// Implement MBC for FileBackedMBC such that the MMU can use this transparently
impl MBC for FileBackedMBC {
    fn readrom(&self, a: u16) -> u8 {
        self.mbc.readrom(a)
    }

    fn readram(&self, a: u16) -> u8 {
        self.mbc.readram(a)
    }

    fn writerom(&mut self, a: u16, v: u8) {
        self.mbc.writerom(a, v)
    }

    fn writeram(&mut self, a: u16, v: u8) {
        self.mbc.writeram(a, v)
    }

    fn is_battery_backed(&self) -> bool {
        self.mbc.is_battery_backed()
    }

    fn loadram(&mut self, ramdata: &[u8]) -> StrResult<()> {
        self.mbc.loadram(ramdata)
    }

    fn dumpram(&self) -> Vec<u8> {
        self.mbc.dumpram()
    }

    fn check_and_reset_ram_updated(&mut self) -> bool {
        self.mbc.check_and_reset_ram_updated()
    }

    fn get_save_path(&self) -> Option<String> {
        Some(self.rampath.clone())
    }

    fn rumble(&self) -> bool {
        self.mbc.rumble()
    }

    fn tick(&mut self, ticks: u32) {
        self.mbc.tick(ticks)
    }

    fn set_tilt(&mut self, x: f32, y: f32) {
        self.mbc.set_tilt(x, y)
    }

    fn take_rom(&mut self) -> Vec<u8> {
        self.mbc.take_rom()
    }

    fn set_rom(&mut self, rom: Vec<u8>) {
        self.mbc.set_rom(rom)
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum Cartridge {
    Memory(MbcState),
    FileBacked(FileBackedMBC),
}

impl Cartridge {
    pub fn from_file(rompath: path::PathBuf, skip_checksum: bool) -> StrResult<Cartridge> {
        FileBackedMBC::new(rompath, skip_checksum).map(Cartridge::FileBacked)
    }

    pub fn from_buffer(data: Vec<u8>, skip_checksum: bool) -> StrResult<Cartridge> {
        get_mbc(data, skip_checksum).map(Cartridge::Memory)
    }

    pub fn from_bytes_with_save(
        data: Vec<u8>,
        save_base: &path::Path,
        skip_checksum: bool,
    ) -> StrResult<Cartridge> {
        FileBackedMBC::from_bytes(data, save_base, skip_checksum).map(Cartridge::FileBacked)
    }

    fn state(&self) -> &MbcState {
        match self {
            Cartridge::Memory(mbc) => mbc,
            Cartridge::FileBacked(fb) => &fb.mbc,
        }
    }

    /// Whether `other` (typically decoded from a save state) has the same
    /// mapper and cartridge-RAM layout. Save states don't carry the ROM, so
    /// this is the guard against loading one game's state onto another.
    pub fn same_layout(&self, other: &Cartridge) -> bool {
        std::mem::discriminant(self.state()) == std::mem::discriminant(other.state())
            && self.dumpram().len() == other.dumpram().len()
    }

    /// Carry what belongs to the physical cartridge, not the saved console
    /// state, over to a cartridge decoded from a save state: the battery-save
    /// path (the state has the one from when it was written) and the RTC,
    /// which keeps running on the cart regardless of console state.
    pub fn adopt_live_state(&mut self, live: &Cartridge) {
        if let (Cartridge::FileBacked(dst), Cartridge::FileBacked(src)) = (&mut *self, live) {
            dst.rampath = src.rampath.clone();
        }
        if let (MbcState::Mbc3(dst), MbcState::Mbc3(src)) = (self.state_mut(), live.state()) {
            dst.set_rtc_state(src.rtc_state());
        }
    }

    fn state_mut(&mut self) -> &mut MbcState {
        match self {
            Cartridge::Memory(mbc) => mbc,
            Cartridge::FileBacked(fb) => &mut fb.mbc,
        }
    }
}

impl MBC for Cartridge {
    fn readrom(&self, a: u16) -> u8 {
        match self {
            Cartridge::Memory(mbc) => mbc.readrom(a),
            Cartridge::FileBacked(mbc) => mbc.readrom(a),
        }
    }

    fn readram(&self, a: u16) -> u8 {
        match self {
            Cartridge::Memory(mbc) => mbc.readram(a),
            Cartridge::FileBacked(mbc) => mbc.readram(a),
        }
    }

    fn writerom(&mut self, a: u16, v: u8) {
        match self {
            Cartridge::Memory(mbc) => mbc.writerom(a, v),
            Cartridge::FileBacked(mbc) => mbc.writerom(a, v),
        }
    }

    fn writeram(&mut self, a: u16, v: u8) {
        match self {
            Cartridge::Memory(mbc) => mbc.writeram(a, v),
            Cartridge::FileBacked(mbc) => mbc.writeram(a, v),
        }
    }

    fn check_and_reset_ram_updated(&mut self) -> bool {
        match self {
            Cartridge::Memory(mbc) => mbc.check_and_reset_ram_updated(),
            Cartridge::FileBacked(mbc) => mbc.check_and_reset_ram_updated(),
        }
    }

    fn is_battery_backed(&self) -> bool {
        match self {
            Cartridge::Memory(mbc) => mbc.is_battery_backed(),
            Cartridge::FileBacked(mbc) => mbc.is_battery_backed(),
        }
    }

    fn loadram(&mut self, ramdata: &[u8]) -> StrResult<()> {
        match self {
            Cartridge::Memory(mbc) => mbc.loadram(ramdata),
            Cartridge::FileBacked(mbc) => mbc.loadram(ramdata),
        }
    }

    fn dumpram(&self) -> Vec<u8> {
        match self {
            Cartridge::Memory(mbc) => mbc.dumpram(),
            Cartridge::FileBacked(mbc) => mbc.dumpram(),
        }
    }

    fn get_save_path(&self) -> Option<String> {
        match self {
            Cartridge::Memory(mbc) => mbc.get_save_path(),
            Cartridge::FileBacked(mbc) => mbc.get_save_path(),
        }
    }

    fn rumble(&self) -> bool {
        match self {
            Cartridge::Memory(mbc) => mbc.rumble(),
            Cartridge::FileBacked(mbc) => mbc.rumble(),
        }
    }

    fn tick(&mut self, ticks: u32) {
        match self {
            Cartridge::Memory(mbc) => mbc.tick(ticks),
            Cartridge::FileBacked(mbc) => mbc.tick(ticks),
        }
    }

    fn set_tilt(&mut self, x: f32, y: f32) {
        match self {
            Cartridge::Memory(mbc) => mbc.set_tilt(x, y),
            Cartridge::FileBacked(mbc) => mbc.set_tilt(x, y),
        }
    }

    fn take_rom(&mut self) -> Vec<u8> {
        match self {
            Cartridge::Memory(mbc) => mbc.take_rom(),
            Cartridge::FileBacked(mbc) => mbc.take_rom(),
        }
    }

    fn set_rom(&mut self, rom: Vec<u8>) {
        match self {
            Cartridge::Memory(mbc) => mbc.set_rom(rom),
            Cartridge::FileBacked(mbc) => mbc.set_rom(rom),
        }
    }
}

/// RAM banks for a cartridge whose type byte says it has RAM. A size byte of 0
/// there is contradictory (Blargg's halt_bug.gb, some homebrew); give it one
/// 8 KiB bank instead of silently dropping every RAM write.
fn ram_banks_with_ram(v: u8) -> usize {
    ram_banks(v).max(1)
}

fn ram_banks(v: u8) -> usize {
    match v {
        1 =>
        // "Listed in various unofficial docs as 2 KiB. However, a 2 KiB RAM chip was never
        // used in a cartridge. The source of this value is unknown."
        // Needed by some test roms. As we only deal in whole banks, just make it 1 8KiB bank.
        {
            1
        }
        2 => 1,
        3 => 4,
        4 => 16,
        5 => 8,
        _ => 0,
    }
}

fn rom_banks(v: u8) -> usize {
    if v <= 8 {
        2 << v
    } else {
        0
    }
}

/// Error returned when the cartridge header checksum doesn't match.
pub const CHECKSUM_INVALID: &str = "Cartridge checksum is invalid";

fn check_checksum(data: &[u8]) -> StrResult<()> {
    let value = data[0x134..0x14D]
        .iter()
        .fold(0u8, |v, &b| v.wrapping_sub(b).wrapping_sub(1));
    match data[0x14D] == value {
        true => Ok(()),
        false => Err(CHECKSUM_INVALID),
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn bank_switching_mbcs_reject_invalid_rom_size_byte() {
        // MBC1/MBC2/MBC5 compute `% rombanks`; a size byte > 8 (rom_banks == 0)
        // must be a clean Err, not a divide-by-zero panic on first bank switch.
        for mbc_type in [0x01u8, 0x05, 0x19] {
            let mut data = vec![0u8; 0x8000];
            data[0x147] = mbc_type;
            data[0x148] = 0x09;
            assert_eq!(
                super::get_mbc(data, true).err(),
                Some("Invalid ROM size in cartridge header"),
                "MBC type {:#04X}",
                mbc_type
            );
        }
    }

    #[test]
    fn non_dividing_mbcs_tolerate_invalid_rom_size_byte() {
        // MBC0 and MBC3 never divide by rombanks; an odd size byte on an
        // otherwise-playable ROM must not be rejected (compatibility).
        for mbc_type in [0x00u8, 0x11] {
            let mut data = vec![0u8; 0x8000];
            data[0x147] = mbc_type;
            data[0x148] = 0x52;
            assert!(
                super::get_mbc(data, true).is_ok(),
                "MBC type {:#04X} should load despite size byte 0x52",
                mbc_type
            );
        }
    }

    #[test]
    fn checksum_zero() {
        let mut data = vec![0; 0x150];
        data[0x14D] = -(0x14D_i32 - 0x134_i32) as u8;
        super::check_checksum(&data).unwrap();
    }

    #[test]
    fn checksum_ones() {
        let mut data = vec![1; 0x150];
        data[0x14D] = (-(0x14D_i32 - 0x134_i32) * 2) as u8;
        super::check_checksum(&data).unwrap();
    }
}
