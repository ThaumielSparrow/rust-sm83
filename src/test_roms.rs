//! Test-ROM harness. Runs the vendored suites in `test/` headlessly:
//!
//! - Mooneye: the test ends with `LD B,B`; pass leaves the Fibonacci
//!   sequence 3/5/8/13/21/34 in B/C/D/E/H/L.
//! - Blargg: result text arrives over serial ("Passed" / "Failed"), and the
//!   newer ROMs also report through cart RAM (signature DE B0 61 at A001,
//!   status at A000, text from A004).
//! - acid2: ends with `LD B,B`; the frame on screen is compared with the
//!   reference PNG.
//!
//! ROMs that don't pass yet are `#[ignore]`d with the reason, so
//! `cargo test -- --ignored` shows accuracy progress while CI stays green and
//! still catches regressions in everything that does pass.

use crate::cpu::CPU;
use crate::gpu::{SCREEN_H, SCREEN_W};
use crate::mbc::Cartridge;

const CLOCK_HZ: u64 = 4_194_304;

#[derive(Debug, PartialEq)]
enum Outcome {
    Pass,
    Fail(String),
    Timeout,
}

struct NullPlayer;

impl crate::AudioPlayer for NullPlayer {
    fn play(&mut self, _: &[f32], _: &[f32]) {}
    fn samples_rate(&self) -> u32 {
        44_100
    }
    fn underflowed(&self) -> bool {
        false
    }
}

/// Build a CPU the way the app does, including an attached APU (the GUI
/// always attaches one, with a null player when there's no audio device).
fn load(path: &str, cgb: bool) -> CPU {
    let rom = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let cart = Cartridge::from_buffer(rom, true).unwrap();
    let mut cpu = if cgb { CPU::new_cgb(cart, None) } else { CPU::new(cart, None) }.unwrap();
    let mut sound = if cpu.mmu.gbmode == crate::gbmode::GbMode::Classic {
        crate::apu::Sound::new_dmg(Box::new(NullPlayer))
    } else {
        crate::apu::Sound::new_cgb(Box::new(NullPlayer))
    };
    sound.apply_post_boot_state();
    cpu.mmu.sound = Some(sound);
    cpu
}

/// Step until the CPU is about to execute `LD B,B` (0x40), or the budget of
/// emulated seconds runs out. Returns false on timeout.
fn run_to_breakpoint(cpu: &mut CPU, seconds: u64) -> bool {
    let budget = CLOCK_HZ * seconds;
    let mut ticks = 0u64;
    while ticks < budget {
        if !cpu.halted && cpu.mmu.rb(cpu.reg.pc) == 0x40 {
            return true;
        }
        ticks += cpu.do_cycle() as u64;
    }
    false
}

fn run_mooneye(path: &str, cgb: bool) -> Outcome {
    let mut cpu = load(path, cgb);
    if !run_to_breakpoint(&mut cpu, 30) {
        return Outcome::Timeout;
    }
    let r = &cpu.reg;
    let regs = [r.b, r.c, r.d, r.e, r.h, r.l];
    if regs == [3, 5, 8, 13, 21, 34] {
        Outcome::Pass
    } else {
        Outcome::Fail(format!("B C D E H L = {regs:02X?}"))
    }
}

fn run_blargg(path: &str, cgb: bool) -> Outcome {
    let mut cpu = load(path, cgb);
    let budget = CLOCK_HZ * 120;
    let mut ticks = 0u64;
    let mut next_check = 0u64;
    while ticks < budget {
        ticks += cpu.do_cycle() as u64;
        if ticks < next_check {
            continue;
        }
        next_check = ticks + CLOCK_HZ / 10;

        let serial = String::from_utf8_lossy(&cpu.mmu.serial.output).into_owned();
        if serial.contains("Passed") {
            return Outcome::Pass;
        }
        if serial.contains("Failed") {
            return Outcome::Fail(serial);
        }
        let sig = [cpu.mmu.rb(0xA001), cpu.mmu.rb(0xA002), cpu.mmu.rb(0xA003)];
        let status = cpu.mmu.rb(0xA000);
        if sig == [0xDE, 0xB0, 0x61] && status != 0x80 {
            if status == 0 {
                return Outcome::Pass;
            }
            let mut text = Vec::new();
            for a in 0xA004..0xBFFF {
                match cpu.mmu.rb(a) {
                    0 => break,
                    b => text.push(b),
                }
            }
            return Outcome::Fail(format!(
                "status {status:02X}: {}",
                String::from_utf8_lossy(&text)
            ));
        }
    }
    eprintln!(
        "{path}: timed out; A000-A003 = {:02X?}, serial = {:?}",
        [cpu.mmu.rb(0xA000), cpu.mmu.rb(0xA001), cpu.mmu.rb(0xA002), cpu.mmu.rb(0xA003)],
        String::from_utf8_lossy(&cpu.mmu.serial.output)
    );
    Outcome::Timeout
}

/// Decode a reference PNG to RGB24.
fn load_reference(path: &str) -> Vec<[u8; 3]> {
    let mut decoder = png::Decoder::new(std::fs::File::open(path).unwrap());
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!((info.width as usize, info.height as usize), (SCREEN_W, SCREEN_H));
    let channels = info.color_type.samples();
    buf.chunks_exact(channels)
        .take(SCREEN_W * SCREEN_H)
        .map(|px| match channels {
            1 | 2 => [px[0]; 3],
            _ => [px[0], px[1], px[2]],
        })
        .collect()
}

/// Compare the screen with the reference up to a consistent recoloring: the
/// emulator's DMG shades and CGB color correction differ from the reference's
/// raw colors, so require a one-to-one mapping between the two palettes
/// instead of equal bytes.
fn compare_frame(frame: &[u8], reference: &[[u8; 3]]) -> Result<(), String> {
    use std::collections::HashMap;
    let mut fwd: HashMap<[u8; 3], [u8; 3]> = HashMap::new();
    let mut back: HashMap<[u8; 3], [u8; 3]> = HashMap::new();
    let mut mismatches = 0usize;
    let mut first = None;
    for (i, want) in reference.iter().enumerate() {
        let got = [frame[i * 3], frame[i * 3 + 1], frame[i * 3 + 2]];
        let ok_fwd = *fwd.entry(*want).or_insert(got) == got;
        let ok_back = *back.entry(got).or_insert(*want) == *want;
        if !(ok_fwd && ok_back) {
            mismatches += 1;
            first.get_or_insert((i % SCREEN_W, i / SCREEN_W));
        }
    }
    match first {
        None => Ok(()),
        Some((x, y)) => Err(format!("{mismatches} pixels differ, first at ({x}, {y})")),
    }
}

fn run_acid2(path: &str, reference: &str, cgb: bool) -> Outcome {
    let mut cpu = load(path, cgb);
    if !run_to_breakpoint(&mut cpu, 10) {
        return Outcome::Timeout;
    }
    // Let the current frame finish and the next one be published.
    for _ in 0..2 {
        while !cpu.mmu.gpu.updated {
            cpu.do_cycle();
        }
        cpu.mmu.gpu.updated = false;
    }
    match compare_frame(cpu.mmu.gpu.front_buffer(), &load_reference(reference)) {
        Ok(()) => Outcome::Pass,
        Err(e) => Outcome::Fail(e),
    }
}

/// One `#[test]` per ROM. `ignore = "..."` marks a known failure.
macro_rules! rom_tests {
    ($runner:ident, $cgb:expr, $dir:literal, [$($(#[$attr:meta])* $name:ident = $file:literal),* $(,)?]) => {
        $(
            #[test]
            $(#[$attr])*
            fn $name() {
                let path = concat!("test/", $dir, "/", $file);
                assert_eq!($runner(path, $cgb), Outcome::Pass, "{}", path);
            }
        )*
    };
}

mod blargg {
    use super::*;
    rom_tests!(run_blargg, false, "", [
        cpu_instrs = "cpu_instrs.gb",
        instr_timing = "instr_timing.gb",
    ]);
    rom_tests!(run_blargg, false, "blargg", [
        mem_timing = "mem_timing.gb",
        mem_timing_2 = "mem_timing-2.gb",
        halt_bug = "halt_bug.gb",
        dmg_sound = "dmg_sound.gb",
    ]);
    mod cgb {
        use super::*;
        rom_tests!(run_blargg, true, "", [
            cpu_instrs = "cpu_instrs.gb",
            instr_timing = "instr_timing.gb",
        ]);
    }
    rom_tests!(run_blargg, true, "blargg", [
        cgb_sound = "cgb_sound.gb",
    ]);
}

mod acid2 {
    use super::*;

    #[test]
    fn dmg_acid2() {
        let r = run_acid2("test/acid2/dmg-acid2.gb", "test/acid2/dmg-acid2-reference.png", false);
        assert_eq!(r, Outcome::Pass);
    }

    #[test]
    fn cgb_acid2() {
        let r = run_acid2("test/acid2/cgb-acid2.gbc", "test/acid2/cgb-acid2-reference.png", true);
        assert_eq!(r, Outcome::Pass);
    }
}

mod mooneye {
    use super::*;

    // Generated from `survey` below. Boot-state ROMs for hardware models we
    // don't emulate (dmg0, MGB, SGB, AGB, cgb0) aren't vendored.
    rom_tests!(run_mooneye, false, "mooneye", [
        acceptance_add_sp_e_timing = "acceptance/add_sp_e_timing.gb",
        acceptance_bits_mem_oam = "acceptance/bits/mem_oam.gb",
        acceptance_bits_reg_f = "acceptance/bits/reg_f.gb",
        acceptance_bits_unused_hwio_gs = "acceptance/bits/unused_hwio-GS.gb",
        acceptance_boot_div_dmgabcmgb = "acceptance/boot_div-dmgABCmgb.gb",
        acceptance_boot_hwio_dmgabcmgb = "acceptance/boot_hwio-dmgABCmgb.gb",
        acceptance_boot_regs_dmgabc = "acceptance/boot_regs-dmgABC.gb",
        acceptance_call_cc_timing = "acceptance/call_cc_timing.gb",
        acceptance_call_cc_timing2 = "acceptance/call_cc_timing2.gb",
        acceptance_call_timing = "acceptance/call_timing.gb",
        acceptance_call_timing2 = "acceptance/call_timing2.gb",
        acceptance_di_timing_gs = "acceptance/di_timing-GS.gb",
        acceptance_div_timing = "acceptance/div_timing.gb",
        acceptance_ei_sequence = "acceptance/ei_sequence.gb",
        acceptance_ei_timing = "acceptance/ei_timing.gb",
        acceptance_halt_ime0_ei = "acceptance/halt_ime0_ei.gb",
        acceptance_halt_ime0_nointr_timing = "acceptance/halt_ime0_nointr_timing.gb",
        acceptance_halt_ime1_timing = "acceptance/halt_ime1_timing.gb",
        acceptance_halt_ime1_timing2_gs = "acceptance/halt_ime1_timing2-GS.gb",
        acceptance_if_ie_registers = "acceptance/if_ie_registers.gb",
        acceptance_instr_daa = "acceptance/instr/daa.gb",
        acceptance_interrupts_ie_push = "acceptance/interrupts/ie_push.gb",
        acceptance_intr_timing = "acceptance/intr_timing.gb",
        acceptance_jp_cc_timing = "acceptance/jp_cc_timing.gb",
        acceptance_jp_timing = "acceptance/jp_timing.gb",
        acceptance_ld_hl_sp_e_timing = "acceptance/ld_hl_sp_e_timing.gb",
        acceptance_oam_dma_basic = "acceptance/oam_dma/basic.gb",
        acceptance_oam_dma_reg_read = "acceptance/oam_dma/reg_read.gb",
        acceptance_oam_dma_sources_gs = "acceptance/oam_dma/sources-GS.gb",
        acceptance_oam_dma_restart = "acceptance/oam_dma_restart.gb",
        acceptance_oam_dma_start = "acceptance/oam_dma_start.gb",
        acceptance_oam_dma_timing = "acceptance/oam_dma_timing.gb",
        acceptance_pop_timing = "acceptance/pop_timing.gb",
        #[ignore = "PPU timing (fixed mode 3 length, STAT edge cases)"] acceptance_ppu_hblank_ly_scx_timing_gs = "acceptance/ppu/hblank_ly_scx_timing-GS.gb",
        acceptance_ppu_intr_1_2_timing_gs = "acceptance/ppu/intr_1_2_timing-GS.gb",
        #[ignore = "PPU timing (fixed mode 3 length, STAT edge cases)"] acceptance_ppu_intr_2_0_timing = "acceptance/ppu/intr_2_0_timing.gb",
        acceptance_ppu_intr_2_mode0_timing = "acceptance/ppu/intr_2_mode0_timing.gb",
        acceptance_ppu_intr_2_mode0_timing_sprites = "acceptance/ppu/intr_2_mode0_timing_sprites.gb",
        acceptance_ppu_intr_2_mode3_timing = "acceptance/ppu/intr_2_mode3_timing.gb",
        acceptance_ppu_intr_2_oam_ok_timing = "acceptance/ppu/intr_2_oam_ok_timing.gb",
        #[ignore = "PPU timing (fixed mode 3 length, STAT edge cases)"] acceptance_ppu_lcdon_timing_gs = "acceptance/ppu/lcdon_timing-GS.gb",
        #[ignore = "PPU timing (fixed mode 3 length, STAT edge cases)"] acceptance_ppu_lcdon_write_timing_gs = "acceptance/ppu/lcdon_write_timing-GS.gb",
        acceptance_ppu_stat_irq_blocking = "acceptance/ppu/stat_irq_blocking.gb",
        acceptance_ppu_stat_lyc_onoff = "acceptance/ppu/stat_lyc_onoff.gb",
        acceptance_ppu_vblank_stat_intr_gs = "acceptance/ppu/vblank_stat_intr-GS.gb",
        acceptance_push_timing = "acceptance/push_timing.gb",
        acceptance_rapid_di_ei = "acceptance/rapid_di_ei.gb",
        acceptance_ret_cc_timing = "acceptance/ret_cc_timing.gb",
        acceptance_ret_timing = "acceptance/ret_timing.gb",
        acceptance_reti_intr_timing = "acceptance/reti_intr_timing.gb",
        acceptance_reti_timing = "acceptance/reti_timing.gb",
        acceptance_rst_timing = "acceptance/rst_timing.gb",
        acceptance_serial_boot_sclk_align_dmgabcmgb = "acceptance/serial/boot_sclk_align-dmgABCmgb.gb",
        acceptance_timer_div_write = "acceptance/timer/div_write.gb",
        #[ignore = "TAC-write glitch needs sub-M-cycle (T-cycle) write timing"] acceptance_timer_rapid_toggle = "acceptance/timer/rapid_toggle.gb",
        acceptance_timer_tim00 = "acceptance/timer/tim00.gb",
        acceptance_timer_tim00_div_trigger = "acceptance/timer/tim00_div_trigger.gb",
        acceptance_timer_tim01 = "acceptance/timer/tim01.gb",
        acceptance_timer_tim01_div_trigger = "acceptance/timer/tim01_div_trigger.gb",
        acceptance_timer_tim10 = "acceptance/timer/tim10.gb",
        acceptance_timer_tim10_div_trigger = "acceptance/timer/tim10_div_trigger.gb",
        acceptance_timer_tim11 = "acceptance/timer/tim11.gb",
        acceptance_timer_tim11_div_trigger = "acceptance/timer/tim11_div_trigger.gb",
        acceptance_timer_tima_reload = "acceptance/timer/tima_reload.gb",
        acceptance_timer_tima_write_reloading = "acceptance/timer/tima_write_reloading.gb",
        acceptance_timer_tma_write_reloading = "acceptance/timer/tma_write_reloading.gb",
        emulator_only_mbc1_bits_bank1 = "emulator-only/mbc1/bits_bank1.gb",
        emulator_only_mbc1_bits_bank2 = "emulator-only/mbc1/bits_bank2.gb",
        emulator_only_mbc1_bits_mode = "emulator-only/mbc1/bits_mode.gb",
        emulator_only_mbc1_bits_ramg = "emulator-only/mbc1/bits_ramg.gb",
        emulator_only_mbc1_multicart_rom_8mb = "emulator-only/mbc1/multicart_rom_8Mb.gb",
        emulator_only_mbc1_ram_256kb = "emulator-only/mbc1/ram_256kb.gb",
        emulator_only_mbc1_ram_64kb = "emulator-only/mbc1/ram_64kb.gb",
        emulator_only_mbc1_rom_1mb = "emulator-only/mbc1/rom_1Mb.gb",
        emulator_only_mbc1_rom_2mb = "emulator-only/mbc1/rom_2Mb.gb",
        emulator_only_mbc1_rom_4mb = "emulator-only/mbc1/rom_4Mb.gb",
        emulator_only_mbc1_rom_512kb = "emulator-only/mbc1/rom_512kb.gb",
        emulator_only_mbc1_rom_8mb = "emulator-only/mbc1/rom_8Mb.gb",
        emulator_only_mbc2_bits_ramg = "emulator-only/mbc2/bits_ramg.gb",
        emulator_only_mbc2_bits_romb = "emulator-only/mbc2/bits_romb.gb",
        emulator_only_mbc2_bits_unused = "emulator-only/mbc2/bits_unused.gb",
        emulator_only_mbc2_ram = "emulator-only/mbc2/ram.gb",
        emulator_only_mbc2_rom_1mb = "emulator-only/mbc2/rom_1Mb.gb",
        emulator_only_mbc2_rom_2mb = "emulator-only/mbc2/rom_2Mb.gb",
        emulator_only_mbc2_rom_512kb = "emulator-only/mbc2/rom_512kb.gb",
        emulator_only_mbc5_rom_1mb = "emulator-only/mbc5/rom_1Mb.gb",
        emulator_only_mbc5_rom_2mb = "emulator-only/mbc5/rom_2Mb.gb",
        emulator_only_mbc5_rom_4mb = "emulator-only/mbc5/rom_4Mb.gb",
        emulator_only_mbc5_rom_512kb = "emulator-only/mbc5/rom_512kb.gb",
        emulator_only_mbc5_rom_8mb = "emulator-only/mbc5/rom_8Mb.gb",
    ]);
    rom_tests!(run_mooneye, true, "mooneye", [
        misc_bits_unused_hwio_c = "misc/bits/unused_hwio-C.gb",
        misc_boot_div_cgbabcde = "misc/boot_div-cgbABCDE.gb",
        misc_boot_hwio_c = "misc/boot_hwio-C.gb",
        misc_boot_regs_cgb = "misc/boot_regs-cgb.gb",
        misc_ppu_vblank_stat_intr_c = "misc/ppu/vblank_stat_intr-C.gb",
    ]);

    /// Not a test: runs every Mooneye ROM and prints a pass/fail table, to
    /// refresh the ignore lists below.
    /// `cargo test --lib mooneye::survey -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn survey() {
        let mut paths = Vec::new();
        fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "gb") {
                    out.push(p.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        walk(std::path::Path::new("test/mooneye"), &mut paths);
        paths.sort();
        let results: Vec<(String, Outcome)> = std::thread::scope(|s| {
            let handles: Vec<_> = paths
                .iter()
                .map(|p| {
                    s.spawn(move || {
                        let cgb = p.contains("/misc/");
                        (p.clone(), run_mooneye(p, cgb))
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (p, r) in &results {
            println!("{:8} {p}", match r {
                Outcome::Pass => "PASS",
                Outcome::Fail(_) => "FAIL",
                Outcome::Timeout => "TIMEOUT",
            });
        }
        let passed = results.iter().filter(|(_, r)| *r == Outcome::Pass).count();
        println!("{passed}/{} passed", results.len());
    }
}

/// Throughput benchmark, not a correctness test:
/// `cargo test --release --lib test_roms::bench -- --ignored --nocapture`
#[test]
#[ignore]
fn bench() {
    for (path, cgb) in [
        ("test/cpu_instrs.gb", false),
        ("test/acid2/dmg-acid2.gb", false),
        ("test/acid2/cgb-acid2.gbc", true),
    ] {
        let mut cpu = load(path, cgb);
        let budget = CLOCK_HZ * 60;
        let start = std::time::Instant::now();
        let mut ticks = 0u64;
        while ticks < budget {
            ticks += cpu.do_cycle() as u64;
        }
        let secs = start.elapsed().as_secs_f64();
        println!("{path}: 60 emulated s in {secs:.3} s ({:.0}x realtime)", 60.0 / secs);
    }
}







