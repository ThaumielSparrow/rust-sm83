#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod cpu;

use cpu::CPU;
use std::fs;

fn main() {
    let mut cpu = CPU::new();
    cpu.init();

    // Try to load a ROM file if provided
    let args: Vec<String> = std::env::args().collect();
    let mut rom_loaded = false;
    if args.len() > 1 {
        match fs::read(&args[1]) {
            Ok(rom_data) => {
                println!("Loading ROM: {}", args[1]);
                cpu.load_rom(&rom_data);
                println!("ROM loaded successfully ({} bytes)", rom_data.len());
                rom_loaded = true;
            }
            Err(e) => {
                println!("Failed to load ROM {}: {}", args[1], e);
                return;
            }
        }
    }

    if !rom_loaded {
        // Load test program into memory when no ROM provided
        cpu.memory.write_byte(0x0100, 0x06); // LD B, n
        cpu.memory.write_byte(0x0101, 0x0A); // n = 0x0A
        cpu.memory.write_byte(0x0102, 0x05); // DEC B
        cpu.memory.write_byte(0x0103, 0xC3); // JP nn
        cpu.memory.write_byte(0x0104, 0x02); // low byte -> 0x0102
        cpu.memory.write_byte(0x0105, 0x01); // high byte -> 0x0102
    }

    println!("Starting CPU emulation...");
    println!("Initial state: PC=0x{:04X}, SP=0x{:04X}, A=0x{:02X}", 
             cpu.registers.pc, cpu.registers.sp, cpu.registers.a);

    // Quiet run: don't print each cycle. If an error (panic) occurs while executing
    // an instruction, catch it and print diagnostic state for that opcode.
    let mut instr_count: usize = 0;
    let max_instructions: usize = 5_000_000; // safety limit to avoid infinite runs

    loop {
        if cpu.halted {
            println!("CPU halted!");
            break;
        }

        instr_count += 1;
        if instr_count > max_instructions {
            println!("Reached max instruction count ({}). Aborting.", max_instructions);
            break;
        }

        // Run the next instruction, but catch panics so we can dump useful state.
        let step_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cpu.step()
        }));

        match step_result {
            Ok(_cycles) => {
                // quiet success; continue
            }
            Err(payload) => {
                // Print a helpful diagnostic: PC, registers, flags and nearby bytes
                eprintln!("ERROR: panic while executing instruction.");
                eprintln!("Panic payload: {:?}", payload);
                eprintln!("PC=0x{:04X} SP=0x{:04X}", cpu.registers.pc, cpu.registers.sp);
                eprintln!("A=0x{:02X} B=0x{:02X} C=0x{:02X} D=0x{:02X} E=0x{:02X} H=0x{:02X} L=0x{:02X}",
                          cpu.registers.a, cpu.registers.b, cpu.registers.c, cpu.registers.d,
                          cpu.registers.e, cpu.registers.h, cpu.registers.l);
                eprintln!("Flags=0x{:02X}", cpu.registers.f);

                // Dump a few bytes around PC to help identify the opcode
                let pc = cpu.registers.pc as usize;
                let mut surrounding = Vec::new();
                for i in pc.saturating_sub(4)..=(pc + 4) {
                    // memory read is safe for ROM range; clamp to 0..=0xFFFF
                    if i <= 0xFFFF {
                        surrounding.push((i as u16, cpu.memory.read_byte(i as u16)));
                    }
                }
                eprintln!("Memory around PC:");
                for (addr, byte) in surrounding {
                    eprintln!("  0x{:04X}: 0x{:02X}", addr, byte);
                }

                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adc_and_sbc_and_flags() {
        let mut cpu = CPU::new();
        cpu.init();
        // ADC A, n (0xCE)
        cpu.registers.a = 0x14;
        cpu.registers.set_flag(cpu::registers::Flag::C, true);
        cpu.memory.write_byte(0x0100, 0xCE); // ADC A, n
        cpu.memory.write_byte(0x0101, 0x22); // n = 0x22
        cpu.registers.pc = 0x0100;
        cpu.step();
        assert_eq!(cpu.registers.a, 0x14u8.wrapping_add(0x22).wrapping_add(1));
        assert!(!cpu.registers.get_flag(cpu::registers::Flag::N));

        // SBC A, n (0xDE)
        cpu.registers.a = 0x50;
        cpu.registers.set_flag(cpu::registers::Flag::C, true);
        cpu.memory.write_byte(0x0102, 0xDE); // SBC A,n
        cpu.memory.write_byte(0x0103, 0x10);
        cpu.registers.pc = 0x0102;
        cpu.step();
        assert_eq!(cpu.registers.a, 0x50u8.wrapping_sub(0x10).wrapping_sub(1));
        assert!(cpu.registers.get_flag(cpu::registers::Flag::N));
    }

    #[test]
    fn test_daa_cpl_scf_ccf() {
        let mut cpu = CPU::new();
        cpu.init();

    // DAA after adding 0x15 + 0x27 -> 0x3C then adjusted by DAA to 0x42
        cpu.registers.a = 0x15;
        cpu.memory.write_byte(0x0100, 0xC6); // ADD A, n
        cpu.memory.write_byte(0x0101, 0x27);
        cpu.memory.write_byte(0x0102, 0x27); // DAA opcode
        cpu.registers.pc = 0x0100;
        cpu.step(); // ADD
        cpu.step(); // DAA
        assert_eq!(cpu.registers.a, 0x42);

        // CPL flips bits
        cpu.registers.a = 0x0F;
        cpu.memory.write_byte(0x0103, 0x2F); // CPL
        cpu.registers.pc = 0x0103;
        cpu.step();
        assert_eq!(cpu.registers.a, 0xF0);

        // SCF sets carry
        cpu.memory.write_byte(0x0104, 0x37); // SCF
        cpu.registers.pc = 0x0104;
        cpu.step();
        assert!(cpu.registers.get_flag(cpu::registers::Flag::C));

        // CCF toggles carry
        cpu.memory.write_byte(0x0105, 0x3F); // CCF
        cpu.registers.pc = 0x0105;
        cpu.step();
        assert!(!cpu.registers.get_flag(cpu::registers::Flag::C));
    }

    #[test]
    fn test_jr_and_jp_hl_and_add_hl() {
        let mut cpu = CPU::new();
        cpu.init();

        // JR n (0x18) relative forward by +2
        cpu.memory.write_byte(0x0100, 0x18);
        cpu.memory.write_byte(0x0101, 0x02u8 as u8);
        cpu.registers.pc = 0x0100;
        cpu.step();
        assert_eq!(cpu.registers.pc, 0x0104);

        // LD HL, nn then ADD HL, BC (0x09)
        cpu.memory.write_byte(0x0103, 0x21); // LD HL, nn
        cpu.memory.write_byte(0x0104, 0x00);
        cpu.memory.write_byte(0x0105, 0x80); // HL = 0x8000
        cpu.registers.pc = 0x0103;
        cpu.step();
        cpu.registers.set_bc(0x0001);
        cpu.memory.write_byte(0x0106, 0x09); // ADD HL, BC
        cpu.registers.pc = 0x0106;
        cpu.step();
        assert_eq!(cpu.registers.get_hl(), 0x8001);

        // JP (HL) - write a jump target into memory at HL and test JP (HL)
        // JP (HL) sets PC to HL
        cpu.registers.set_hl(0x9000);
        cpu.memory.write_byte(0x0107, 0xE9); // JP (HL)
        cpu.registers.pc = 0x0107;
        cpu.step();
        assert_eq!(cpu.registers.pc, 0x9000);
    }
}