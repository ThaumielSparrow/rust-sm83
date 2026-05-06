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

pub trait MBC: Send {
    fn readrom(&self, a: u16) -> u8;
    fn readram(&self, a: u16) -> u8;
    fn writerom(&mut self, a: u16, v: u8);
    fn writeram(&mut self, a: u16, v: u8);
    fn check_and_reset_ram_updated(&mut self) -> bool;

    fn is_battery_backed(&self) -> bool;
    fn loadram(&mut self, ramdata: &[u8]) -> StrResult<()>;
    fn dumpram(&self) -> Vec<u8>;

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
}

impl MBC for MbcState {
    fn readrom(&self, a: u16) -> u8 {
        match self {
            MbcState::Mbc0(mbc) => mbc.readrom(a),
            MbcState::Mbc1(mbc) => mbc.readrom(a),
            MbcState::Mbc2(mbc) => mbc.readrom(a),
            MbcState::Mbc3(mbc) => mbc.readrom(a),
            MbcState::Mbc5(mbc) => mbc.readrom(a),
        }
    }

    fn readram(&self, a: u16) -> u8 {
        match self {
            MbcState::Mbc0(mbc) => mbc.readram(a),
            MbcState::Mbc1(mbc) => mbc.readram(a),
            MbcState::Mbc2(mbc) => mbc.readram(a),
            MbcState::Mbc3(mbc) => mbc.readram(a),
            MbcState::Mbc5(mbc) => mbc.readram(a),
        }
    }

    fn writerom(&mut self, a: u16, v: u8) {
        match self {
            MbcState::Mbc0(mbc) => mbc.writerom(a, v),
            MbcState::Mbc1(mbc) => mbc.writerom(a, v),
            MbcState::Mbc2(mbc) => mbc.writerom(a, v),
            MbcState::Mbc3(mbc) => mbc.writerom(a, v),
            MbcState::Mbc5(mbc) => mbc.writerom(a, v),
        }
    }

    fn writeram(&mut self, a: u16, v: u8) {
        match self {
            MbcState::Mbc0(mbc) => mbc.writeram(a, v),
            MbcState::Mbc1(mbc) => mbc.writeram(a, v),
            MbcState::Mbc2(mbc) => mbc.writeram(a, v),
            MbcState::Mbc3(mbc) => mbc.writeram(a, v),
            MbcState::Mbc5(mbc) => mbc.writeram(a, v),
        }
    }

    fn check_and_reset_ram_updated(&mut self) -> bool {
        match self {
            MbcState::Mbc0(mbc) => mbc.check_and_reset_ram_updated(),
            MbcState::Mbc1(mbc) => mbc.check_and_reset_ram_updated(),
            MbcState::Mbc2(mbc) => mbc.check_and_reset_ram_updated(),
            MbcState::Mbc3(mbc) => mbc.check_and_reset_ram_updated(),
            MbcState::Mbc5(mbc) => mbc.check_and_reset_ram_updated(),
        }
    }

    fn is_battery_backed(&self) -> bool {
        match self {
            MbcState::Mbc0(mbc) => mbc.is_battery_backed(),
            MbcState::Mbc1(mbc) => mbc.is_battery_backed(),
            MbcState::Mbc2(mbc) => mbc.is_battery_backed(),
            MbcState::Mbc3(mbc) => mbc.is_battery_backed(),
            MbcState::Mbc5(mbc) => mbc.is_battery_backed(),
        }
    }

    fn loadram(&mut self, ramdata: &[u8]) -> StrResult<()> {
        match self {
            MbcState::Mbc0(mbc) => mbc.loadram(ramdata),
            MbcState::Mbc1(mbc) => mbc.loadram(ramdata),
            MbcState::Mbc2(mbc) => mbc.loadram(ramdata),
            MbcState::Mbc3(mbc) => mbc.loadram(ramdata),
            MbcState::Mbc5(mbc) => mbc.loadram(ramdata),
        }
    }

    fn dumpram(&self) -> Vec<u8> {
        match self {
            MbcState::Mbc0(mbc) => mbc.dumpram(),
            MbcState::Mbc1(mbc) => mbc.dumpram(),
            MbcState::Mbc2(mbc) => mbc.dumpram(),
            MbcState::Mbc3(mbc) => mbc.dumpram(),
            MbcState::Mbc5(mbc) => mbc.dumpram(),
        }
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
        0x00 => mbc0::MBC0::new(data).map(MbcState::Mbc0),
        0x01..=0x03 => mbc1::MBC1::new(data).map(MbcState::Mbc1),
        0x05..=0x06 => mbc2::MBC2::new(data).map(MbcState::Mbc2),
        0x0F..=0x13 => mbc3::MBC3::new(data).map(MbcState::Mbc3),
        0x19..=0x1E => mbc5::MBC5::new(data).map(MbcState::Mbc5),
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
        let mut mbc = get_mbc(data, skip_checksum)?;

        let rampath = rompath.with_extension("gbsave");
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
}

impl Drop for FileBackedMBC {
    fn drop(&mut self) {
        if self.mbc.is_battery_backed() {
            // TODO: error handling
            let mut file = match fs::File::create(&self.rampath) {
                Ok(f) => f,
                Err(..) => return,
            };
            let _ = file.write_all(&self.mbc.dumpram());
        }
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

fn check_checksum(data: &[u8]) -> StrResult<()> {
    let mut value: u8 = 0;
    for i in 0x134..0x14D {
        value = value.wrapping_sub(data[i]).wrapping_sub(1);
    }
    match data[0x14D] == value {
        true => Ok(()),
        false => Err("Cartridge checksum is invalid"),
    }
}

#[cfg(test)]
mod test {
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
