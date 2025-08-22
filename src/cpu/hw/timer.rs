// Simple SM83 timer implementation
// Implements DIV (FF04), TIMA (FF05), TMA (FF06), TAC (FF07)

use crate::cpu::mmu::Memory;

pub struct Timer {
    // Divider internal clock (increments at 16384 Hz => every 256 cycles)
    pub div_counter: u16,
}

impl Timer {
    pub fn new() -> Self {
        Timer { div_counter: 0 }
    }

    // Called with number of CPU cycles executed; updates DIV and TIMA according to TAC
    pub fn tick(&mut self, mem: &mut Memory, cycles: u8) {
        // Increment DIV by cycles (DIV is 16-bit internal, but register stores high 8 bits)
        self.div_counter = self.div_counter.wrapping_add(cycles as u16);
        let div_reg = (self.div_counter >> 8) as u8; // DIV high byte
        mem.io_registers[0x04] = div_reg; // 0xFF04

        // Read TAC to determine if timer enabled and frequency
        let tac = mem.io_registers[0x07]; // 0xFF07
        let timer_enabled = (tac & 0x04) != 0;
        let input_clock_select = tac & 0x03;

        if !timer_enabled {
            return;
        }

        // Determine how many internal cycles per TIMA increment based on TAC
        // Game Boy: 00=4096Hz (1024 cycles), 01=262144Hz (16 cycles), 10=65536Hz (64 cycles), 11=16384Hz (256 cycles)
        let threshold: u16 = match input_clock_select {
            0 => 1024,
            1 => 16,
            2 => 64,
            3 => 256,
            _ => 1024,
        };

        // Maintain a TIMA internal counter in high bits of div_counter mod threshold
        // Simpler: keep a separate counter in memory's io_registers[0x70] (unused) to track ticks
        let counter_index = 0x70usize; // spare internal counter slot in io_registers
        let mut internal = mem.io_registers[counter_index] as u32;
        internal = internal.wrapping_add(cycles as u32);

        if internal as u16 >= threshold {
            // subtract threshold and increment TIMA
            internal = internal.wrapping_sub(threshold as u32);
            let tima = mem.io_registers[0x05]; // FF05
            if tima == 0xFF {
                // overflow: set TIMA to TMA and request timer interrupt (bit 2 of IF)
                mem.io_registers[0x05] = mem.io_registers[0x06]; // copy TMA to TIMA
                // set IF bit 2
                mem.io_registers[0x0F] |= 1 << 2;
            } else {
                mem.io_registers[0x05] = tima.wrapping_add(1);
            }
        }

        mem.io_registers[counter_index] = internal as u8;
    }

}

