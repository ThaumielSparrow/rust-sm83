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
    if args.len() > 1 {
        match fs::read(&args[1]) {
            Ok(rom_data) => {
                println!("Loading ROM: {}", args[1]);
                cpu.load_rom(&rom_data);
                println!("ROM loaded successfully ({} bytes)", rom_data.len());
            }
            Err(e) => {
                println!("Failed to load ROM {}: {}", args[1], e);
                return;
            }
        }
    } else {
        // Load test prgram
        // cpu.memory.write_byte(0x0100, 0x3E); // LD A, 0x42
        // cpu.memory.write_byte(0x0101, 0x42);
        // cpu.memory.write_byte(0x0102, 0x06); // LD B, 0x10
        // cpu.memory.write_byte(0x0103, 0x10);
        // cpu.memory.write_byte(0x0104, 0x80); // ADD A, B
        // cpu.memory.write_byte(0x0105, 0x05); // DEC B
        // cpu.memory.write_byte(0x0106, 0xC3); // JP 0x0104 (infinite loop)
        // cpu.memory.write_byte(0x0107, 0x04);
    // Test program: LD B,0x10; DEC B; JP 0x0102 (loop)
    cpu.memory.write_byte(0x0100, 0x06); // LD B, n
    cpu.memory.write_byte(0x0101, 0x0A); // n = 0x10
    cpu.memory.write_byte(0x0102, 0x05); // DEC B
    cpu.memory.write_byte(0x0103, 0xC3); // JP nn
    cpu.memory.write_byte(0x0104, 0x02); // low byte -> 0x0102
    cpu.memory.write_byte(0x0105, 0x01); // high byte -> 0x0102
    }

    println!("Starting CPU emulation...");
    println!("Initial state: PC=0x{:04X}, SP=0x{:04X}, A=0x{:02X}", 
             cpu.registers.pc, cpu.registers.sp, cpu.registers.a);

    // Run emulation loop
    for cycle in 0..10 {
        let cycles = cpu.step();
        println!("Cycle {}: PC=0x{:04X}, A=0x{:02X}, B=0x{:02X}, Flags=0x{:02X}, Cycles={}", 
                 cycle + 1, cpu.registers.pc, cpu.registers.a, cpu.registers.b, 
                 cpu.registers.f, cycles);

        // Break if we hit a halt
        if cpu.halted {
            println!("CPU halted!");
            break;
        }
    }
}