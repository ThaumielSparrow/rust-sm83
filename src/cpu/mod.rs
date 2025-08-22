pub mod registers;
pub mod mmu;

use mmu::Memory;
use registers::Registers;

pub struct CPU {
    pub registers: Registers,
    pub memory: Memory, // 64KB memory space
    pub halted: bool,
    pub ime: bool, // Interrupt Master Enable
}

impl CPU {
    pub fn new() -> Self {
        CPU {
            registers: Registers::new(),
            memory: Memory::new(),
            halted: false,
            ime: false,
        }
    }

    pub fn init(&mut self) {
        // Set initial register values to after boot ROM state
        self.registers.a = 0x01;
        self.registers.f = 0xB0;
        self.registers.b = 0x00;
        self.registers.c = 0x13;
        self.registers.d = 0x00;
        self.registers.e = 0xD8;
        self.registers.h = 0x01;
        self.registers.l = 0x4D;
        self.registers.pc = 0x0100; // Start after boot ROM
        self.registers.sp = 0xFFFE;
    }

    pub fn step(&mut self) -> u8 {
        if self.halted {
            return 4; // NOP timing when halted
        }

        let opcode = self.fetch_byte();

        if opcode == 0xCB {
            let cb_opcode = self.fetch_byte();
            self.execute_cb_instruction(cb_opcode)
        } else {
            self.execute_instruction(opcode)
        }
    }

    // Fetch next byte from memory at PC and increment PC
    fn fetch_byte(&mut self) -> u8 {
        let byte = self.memory.read_byte(self.registers.pc);
        self.registers.pc = self.registers.pc.wrapping_add(1);
        byte
    }

    // Fetch next 16-bit word from memory at PC and increment PC (by 2)
    fn fetch_word(&mut self) -> u16 {
        let low = self.fetch_byte() as u16;
        let high = self.fetch_byte() as u16;
        (high << 8) | low
    }

    // Execute instruction based on opcode
    fn execute_instruction(&mut self, opcode: u8) -> u8 {
        match opcode {
            // 8-bit loads
            0x06 => { self.registers.b = self.fetch_byte(); 8 }  // LD B, n
            0x0E => { self.registers.c = self.fetch_byte(); 8 }  // LD C, n
            0x16 => { self.registers.d = self.fetch_byte(); 8 }  // LD D, n
            0x1E => { self.registers.e = self.fetch_byte(); 8 }  // LD E, n
            0x26 => { self.registers.h = self.fetch_byte(); 8 }  // LD H, n
            0x2E => { self.registers.l = self.fetch_byte(); 8 }  // LD L, n
            0x36 => { 
                let addr = self.registers.get_hl();
                let value = self.fetch_byte();
                self.memory.write_byte(addr, value);
                12
            } // LD (HL), n
            0x3E => { self.registers.a = self.fetch_byte(); 8 }  // LD A, n

            // 8-bit register to register loads
            0x40..=0x7F => {
                let src = opcode & 0x07;
                let dst = (opcode >> 3) & 0x07;
                
                if opcode == 0x76 { // HALT
                    self.halted = true;
                    return 4;
                }
                
                let value = self.get_r8(src);
                self.set_r8(dst, value);
                
                if src == 6 || dst == 6 { 8 } else { 4 } // (HL) takes extra cycles
            }

            // 16-bit loads
            0x01 => { let val = self.fetch_word(); self.registers.set_bc(val); 12 } // LD BC, nn
            0x11 => { let val = self.fetch_word(); self.registers.set_de(val); 12 } // LD DE, nn  
            0x21 => { let val = self.fetch_word(); self.registers.set_hl(val); 12 } // LD HL, nn
            0x31 => { self.registers.sp = self.fetch_word(); 12 }                  // LD SP, nn

            // Memory loads
            0x02 => { self.memory.write_byte(self.registers.get_bc(), self.registers.a); 8 } // LD (BC), A
            0x0A => { self.registers.a = self.memory.read_byte(self.registers.get_bc()); 8 } // LD A, (BC)
            0x12 => { self.memory.write_byte(self.registers.get_de(), self.registers.a); 8 } // LD (DE), A
            0x1A => { self.registers.a = self.memory.read_byte(self.registers.get_de()); 8 } // LD A, (DE)

            // HL increment/decrement loads
            0x22 => { // LD (HL+), A - Load A into (HL) and increment HL
                self.memory.write_byte(self.registers.get_hl(), self.registers.a);
                let hl = self.registers.get_hl().wrapping_add(1);
                self.registers.set_hl(hl);
                8
            }
            0x2A => { // LD A, (HL+) - Load (HL) into A and increment HL
                self.registers.a = self.memory.read_byte(self.registers.get_hl());
                let hl = self.registers.get_hl().wrapping_add(1);
                self.registers.set_hl(hl);
                8
            }
            0x32 => { // LD (HL-), A - Load A into (HL) and decrement HL
                self.memory.write_byte(self.registers.get_hl(), self.registers.a);
                let hl = self.registers.get_hl().wrapping_sub(1);
                self.registers.set_hl(hl);
                8
            }
            0x3A => { // LD A, (HL-) - Load (HL) into A and decrement HL
                self.registers.a = self.memory.read_byte(self.registers.get_hl());
                let hl = self.registers.get_hl().wrapping_sub(1);
                self.registers.set_hl(hl);
                8
            }

            // 8-bit arithmetic
            0x04 => { self.registers.b = self.inc_8bit(self.registers.b); 4 }    // INC B
            0x05 => { self.registers.b = self.dec_8bit(self.registers.b); 4 }    // DEC B
            0x0C => { self.registers.c = self.inc_8bit(self.registers.c); 4 }    // INC C
            0x0D => { self.registers.c = self.dec_8bit(self.registers.c); 4 }    // DEC C
            0x14 => { self.registers.d = self.inc_8bit(self.registers.d); 4 }    // INC D
            0x15 => { self.registers.d = self.dec_8bit(self.registers.d); 4 }    // DEC D
            0x1C => { self.registers.e = self.inc_8bit(self.registers.e); 4 }    // INC E
            0x1D => { self.registers.e = self.dec_8bit(self.registers.e); 4 }    // DEC E
            0x24 => { self.registers.h = self.inc_8bit(self.registers.h); 4 }    // INC H
            0x25 => { self.registers.h = self.dec_8bit(self.registers.h); 4 }    // DEC H
            0x2C => { self.registers.l = self.inc_8bit(self.registers.l); 4 }    // INC L
            0x2D => { self.registers.l = self.dec_8bit(self.registers.l); 4 }    // DEC L
            0x34 => { // INC (HL)
                let addr = self.registers.get_hl();
                let value = self.memory.read_byte(addr);
                let result = self.inc_8bit(value);
                self.memory.write_byte(addr, result);
                12
            }
            0x35 => { // DEC (HL)
                let addr = self.registers.get_hl();
                let value = self.memory.read_byte(addr);
                let result = self.dec_8bit(value);
                self.memory.write_byte(addr, result);
                12
            }
            0x3C => { self.registers.a = self.inc_8bit(self.registers.a); 4 }    // INC A
            0x3D => { self.registers.a = self.dec_8bit(self.registers.a); 4 }    // DEC A

            // ADD A, r8
            0x80..=0x87 => {
                let src = opcode & 0x07;
                let value = self.get_r8(src);
                self.add_a(value);
                if src == 6 { 8 } else { 4 }
            }

            // ADC A, r8
            0x88..=0x8F => {
                let src = opcode & 0x07;
                let value = self.get_r8(src);
                self.adc_a(value);
                if src == 6 { 8 } else { 4 }
            }

            // SUB r8
            0x90..=0x97 => {
                let src = opcode & 0x07;
                let value = self.get_r8(src);
                self.sub_a(value);
                if src == 6 { 8 } else { 4 }
            }
            // SBC A, r8
            0x98..=0x9F => {
                let src = opcode & 0x07;
                let value = self.get_r8(src);
                self.sbc_a(value);
                if src == 6 { 8 } else { 4 }
            }

            // AND r8
            0xA0..=0xA7 => {
                let src = opcode & 0x07;
                let value = self.get_r8(src);
                self.and_a(value);
                if src == 6 { 8 } else { 4 }
            }

            // XOR r8
            0xA8..=0xAF => {
                let src = opcode & 0x07;
                let value = self.get_r8(src);
                self.xor_a(value);
                if src == 6 { 8 } else { 4 }
            }

            // OR r8
            0xB0..=0xB7 => {
                let src = opcode & 0x07;
                let value = self.get_r8(src);
                self.or_a(value);
                if src == 6 { 8 } else { 4 }
            }

            // CP r8 (Compare)
            0xB8..=0xBF => {
                let src = opcode & 0x07;
                let value = self.get_r8(src);
                self.cp_a(value);
                if src == 6 { 8 } else { 4 }
            }

            // 16-bit arithmetic
            0x03 => { let bc = self.registers.get_bc().wrapping_add(1); self.registers.set_bc(bc); 8 } // INC BC
            0x0B => { let bc = self.registers.get_bc().wrapping_sub(1); self.registers.set_bc(bc); 8 } // DEC BC
            0x13 => { let de = self.registers.get_de().wrapping_add(1); self.registers.set_de(de); 8 } // INC DE
            0x1B => { let de = self.registers.get_de().wrapping_sub(1); self.registers.set_de(de); 8 } // DEC DE
            0x23 => { let hl = self.registers.get_hl().wrapping_add(1); self.registers.set_hl(hl); 8 } // INC HL
            0x2B => { let hl = self.registers.get_hl().wrapping_sub(1); self.registers.set_hl(hl); 8 } // DEC HL
            0x33 => { self.registers.sp = self.registers.sp.wrapping_add(1); 8 }                       // INC SP
            0x3B => { self.registers.sp = self.registers.sp.wrapping_sub(1); 8 }                       // DEC SP

            // ADD HL, rr (16-bit add)
            0x09 => { self.add_hl(self.registers.get_bc()); 8 }
            0x19 => { self.add_hl(self.registers.get_de()); 8 }
            0x29 => { self.add_hl(self.registers.get_hl()); 8 }
            0x39 => { self.add_hl(self.registers.sp); 8 }

            // LD (a16), SP - store SP at immediate 16-bit address (low then high)
            0x08 => {
                let addr = self.fetch_word();
                let sp = self.registers.sp;
                self.memory.write_byte(addr, (sp & 0xFF) as u8);
                self.memory.write_byte(addr.wrapping_add(1), (sp >> 8) as u8);
                20
            }

            // Jumps and calls
            0xC3 => { self.registers.pc = self.fetch_word(); 16 }    // JP nn
            0xE9 => { self.registers.pc = self.registers.get_hl(); 4 } // JP (HL)

            // Relative jumps
            0x18 => { // JR n (always)
                let offset = self.fetch_byte() as i8;
                let new_pc = ((self.registers.pc as i32) + (offset as i32)) as u16;
                self.registers.pc = new_pc;
                12
            }
            0x20 => { // JR NZ,n
                let offset = self.fetch_byte() as i8;
                if !self.registers.get_flag(registers::Flag::Z) {
                    self.registers.pc = ((self.registers.pc as i32) + (offset as i32)) as u16;
                    12
                } else { 8 }
            }
            0x28 => { // JR Z,n
                let offset = self.fetch_byte() as i8;
                if self.registers.get_flag(registers::Flag::Z) {
                    self.registers.pc = ((self.registers.pc as i32) + (offset as i32)) as u16;
                    12
                } else { 8 }
            }
            0x30 => { // JR NC,n
                let offset = self.fetch_byte() as i8;
                if !self.registers.get_flag(registers::Flag::C) {
                    self.registers.pc = ((self.registers.pc as i32) + (offset as i32)) as u16;
                    12
                } else { 8 }
            }
            0x38 => { // JR C,n
                let offset = self.fetch_byte() as i8;
                if self.registers.get_flag(registers::Flag::C) {
                    self.registers.pc = ((self.registers.pc as i32) + (offset as i32)) as u16;
                    12
                } else { 8 }
            }
            0xC2 => self.jp_cond(!self.registers.get_flag(registers::Flag::Z)), // JP NZ, nn
            0xCA => self.jp_cond(self.registers.get_flag(registers::Flag::Z)),  // JP Z, nn
            0xD2 => self.jp_cond(!self.registers.get_flag(registers::Flag::C)), // JP NC, nn
            0xDA => self.jp_cond(self.registers.get_flag(registers::Flag::C)),  // JP C, nn

            0xCD => self.call(),                                                // CALL nn
            0xC4 => self.call_cond(!self.registers.get_flag(registers::Flag::Z)), // CALL NZ, nn
            0xCC => self.call_cond(self.registers.get_flag(registers::Flag::Z)),  // CALL Z, nn
            0xD4 => self.call_cond(!self.registers.get_flag(registers::Flag::C)), // CALL NC, nn
            0xDC => self.call_cond(self.registers.get_flag(registers::Flag::C)),  // CALL C, nn

            0xC9 => self.ret(),                                                 // RET
            0xC0 => self.ret_cond(!self.registers.get_flag(registers::Flag::Z)), // RET NZ
            0xC8 => self.ret_cond(self.registers.get_flag(registers::Flag::Z)),  // RET Z
            0xD0 => self.ret_cond(!self.registers.get_flag(registers::Flag::C)), // RET NC
            0xD8 => self.ret_cond(self.registers.get_flag(registers::Flag::C)),  // RET C

            // Stack operations
            0xC1 => { let val = self.pop(); self.registers.set_bc(val); 12 } // POP BC
            0xC5 => { let val = self.registers.get_bc(); self.push(val); 16 } // PUSH BC
            0xD1 => { let val = self.pop(); self.registers.set_de(val); 12 } // POP DE
            0xD5 => { let val = self.registers.get_de(); self.push(val); 16 } // PUSH DE
            0xE1 => { let val = self.pop(); self.registers.set_hl(val); 12 } // POP HL
            0xE5 => { let val = self.registers.get_hl(); self.push(val); 16 } // PUSH HL
            0xF1 => { let val = self.pop(); self.registers.set_af(val); 12 } // POP AF
            0xF5 => { let val = self.registers.get_af(); self.push(val); 16 } // PUSH AF

            // Rotates and shifts
            0x07 => { self.rlca(); 4 }  // RLCA
            0x0F => { self.rrca(); 4 }  // RRCA
            0x17 => { self.rla(); 4 }   // RLA
            0x1F => { self.rra(); 4 }   // RRA
            0x27 => { self.daa(); 4 }   // DAA
            0x2F => { self.cpl(); 4 }   // CPL
            0x37 => { self.scf(); 4 }   // SCF
            0x3F => { self.ccf(); 4 }   // CCF

            // Immediate arithmetic
            0xC6 => { let val = self.fetch_byte(); self.add_a(val); 8 }  // ADD A, n
            0xCE => { let val = self.fetch_byte(); self.adc_a(val); 8 }  // ADC A, n
            0xDE => { let val = self.fetch_byte(); self.sbc_a(val); 8 }  // SBC A, n
            0xD6 => { let val = self.fetch_byte(); self.sub_a(val); 8 }  // SUB n
            0xE6 => { let val = self.fetch_byte(); self.and_a(val); 8 }  // AND n
            0xEE => { let val = self.fetch_byte(); self.xor_a(val); 8 }  // XOR n
            0xF6 => { let val = self.fetch_byte(); self.or_a(val); 8 }   // OR n
            0xFE => { let val = self.fetch_byte(); self.cp_a(val); 8 }   // CP n

            // Misc arithmetic and special ops
            0xE8 => { // ADD SP, e (signed immediate)
                let offset = self.fetch_byte() as i8 as i16 as i32;
                let result = (self.registers.sp as i32).wrapping_add(offset) as u16;
                // Flags: Z = 0, N = 0, H and C based on 8-bit addition of low byte
                let low_sp = (self.registers.sp & 0xFF) as u8;
                let offset8 = offset as i8 as u8;
                let half = ((low_sp & 0x0F) as u16 + (offset8 & 0x0F) as u16) > 0x0F;
                let carry = ((low_sp as u16) + (offset8 as u16)) > 0xFF;
                self.registers.sp = result;
                self.registers.set_flag(registers::Flag::Z, false);
                self.registers.set_flag(registers::Flag::N, false);
                self.registers.set_flag(registers::Flag::H, half);
                self.registers.set_flag(registers::Flag::C, carry);
                16
            }

            0xF8 => { // LD HL, SP + e
                let offset = self.fetch_byte() as i8 as i16 as i32;
                let result = (self.registers.sp as i32).wrapping_add(offset) as u16;
                let low_sp = (self.registers.sp & 0xFF) as u8;
                let offset8 = offset as i8 as u8;
                let half = ((low_sp & 0x0F) as u16 + (offset8 & 0x0F) as u16) > 0x0F;
                let carry = ((low_sp as u16) + (offset8 as u16)) > 0xFF;
                self.registers.set_hl(result);
                self.registers.set_flag(registers::Flag::Z, false);
                self.registers.set_flag(registers::Flag::N, false);
                self.registers.set_flag(registers::Flag::H, half);
                self.registers.set_flag(registers::Flag::C, carry);
                12
            }

            0xF9 => { // LD SP, HL
                self.registers.sp = self.registers.get_hl();
                8
            }

            // Memory loads with immediate address
            0xEA => { // LD (nn), A
                let addr = self.fetch_word();
                self.memory.write_byte(addr, self.registers.a);
                16
            }
            0xFA => { // LD A, (nn)
                let addr = self.fetch_word();
                self.registers.a = self.memory.read_byte(addr);
                16
            }

            // High page loads (0xFF00 + n)
            0xE0 => { // LDH (n), A
                let addr = 0xFF00 + self.fetch_byte() as u16;
                self.memory.write_byte(addr, self.registers.a);
                12
            }
            0xF0 => { // LDH A, (n)
                let addr = 0xFF00 + self.fetch_byte() as u16;
                self.registers.a = self.memory.read_byte(addr);
                12
            }
            0xE2 => { // LD (C), A
                let addr = 0xFF00 + self.registers.c as u16;
                self.memory.write_byte(addr, self.registers.a);
                8
            }
            0xF2 => { // LD A, (C)
                let addr = 0xFF00 + self.registers.c as u16;
                self.registers.a = self.memory.read_byte(addr);
                8
            }

            // Interrupt control
            0xF3 => { self.ime = false; 4 } // DI
            0xFB => { self.ime = true; 4 }  // EI

            // Returns and resets
            0xD9 => { // RETI - return and enable interrupts
                let pc = self.pop();
                self.registers.pc = pc;
                self.ime = true;
                16
            }

            0xC7 | 0xCF | 0xD7 | 0xDF | 0xE7 | 0xEF | 0xF7 | 0xFF => { // RST n
                let vector = match opcode {
                    0xC7 => 0x00,
                    0xCF => 0x08,
                    0xD7 => 0x10,
                    0xDF => 0x18,
                    0xE7 => 0x20,
                    0xEF => 0x28,
                    0xF7 => 0x30,
                    0xFF => 0x38,
                    _ => 0x00,
                };
                self.push(self.registers.pc);
                self.registers.pc = vector;
                16
            }

            // Misc
            0x00 => 4,    // NOP
            0x10 => 4,    // STOP
            _ => panic!("Unimplemented instruction: 0x{:02X} at PC: 0x{:04X}", opcode, self.registers.pc - 1),
        }
    }

    fn execute_cb_instruction(&mut self, opcode: u8) -> u8 {
        let reg_index = opcode & 0x07;
        let bit = (opcode >> 3) & 0x07;
        let op = opcode >> 6;

        match op {
            0 => { // Rotates and shifts
                let value = self.get_r8(reg_index);
                let result = match (opcode >> 3) & 0x07 {
                    0 => self.rlc(value),  // RLC
                    1 => self.rrc(value),  // RRC
                    2 => self.rl(value),   // RL
                    3 => self.rr(value),   // RR
                    4 => self.sla(value),  // SLA
                    5 => self.sra(value),  // SRA
                    6 => self.swap(value), // SWAP
                    7 => self.srl(value),  // SRL
                    _ => unreachable!(),
                };
                self.set_r8(reg_index, result);
                if reg_index == 6 { 16 } else { 8 }
            }
            1 => { // BIT
                let value = self.get_r8(reg_index);
                let bit_set = (value & (1 << bit)) != 0;
                self.registers.set_flag(registers::Flag::Z, !bit_set);
                self.registers.set_flag(registers::Flag::N, false);
                self.registers.set_flag(registers::Flag::H, true);
                if reg_index == 6 { 12 } else { 8 }
            }
            2 => { // RES
                let value = self.get_r8(reg_index);
                let result = value & !(1 << bit);
                self.set_r8(reg_index, result);
                if reg_index == 6 { 16 } else { 8 }
            }
            3 => { // SET
                let value = self.get_r8(reg_index);
                let result = value | (1 << bit);
                self.set_r8(reg_index, result);
                if reg_index == 6 { 16 } else { 8 }
            }
            _ => unreachable!(),
        }
    }

    // Helper functions for register access
    fn get_r8(&self, index: u8) -> u8 {
        match index {
            0 => self.registers.b,
            1 => self.registers.c,
            2 => self.registers.d,
            3 => self.registers.e,
            4 => self.registers.h,
            5 => self.registers.l,
            6 => self.memory.read_byte(self.registers.get_hl()),
            7 => self.registers.a,
            _ => unreachable!(),
        }
    }

    fn set_r8(&mut self, index: u8, value: u8) {
        match index {
            0 => self.registers.b = value,
            1 => self.registers.c = value,
            2 => self.registers.d = value,
            3 => self.registers.e = value,
            4 => self.registers.h = value,
            5 => self.registers.l = value,
            6 => self.memory.write_byte(self.registers.get_hl(), value),
            7 => self.registers.a = value,
            _ => unreachable!(),
        }
    }

    // Arithmetic operations
    fn inc_8bit(&mut self, value: u8) -> u8 {
        let result = value.wrapping_add(1);
        self.registers.set_flag(registers::Flag::Z, result == 0);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, (value & 0x0F) == 0x0F);
        result
    }

    fn dec_8bit(&mut self, value: u8) -> u8 {
        let result = value.wrapping_sub(1);
        self.registers.set_flag(registers::Flag::Z, result == 0);
        self.registers.set_flag(registers::Flag::N, true);
        self.registers.set_flag(registers::Flag::H, (value & 0x0F) == 0);
        result
    }

    fn add_a(&mut self, value: u8) {
        let result = self.registers.a.wrapping_add(value);
        let carry = (self.registers.a as u16 + value as u16) > 0xFF;
        let half_carry = (self.registers.a & 0x0F) + (value & 0x0F) > 0x0F;
        
        self.registers.a = result;
        self.registers.set_flag(registers::Flag::Z, result == 0);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, half_carry);
        self.registers.set_flag(registers::Flag::C, carry);
    }

    fn adc_a(&mut self, value: u8) {
        let carry = if self.registers.get_flag(registers::Flag::C) { 1 } else { 0 };
        let result = self.registers.a.wrapping_add(value).wrapping_add(carry);
        let full_carry = (self.registers.a as u16 + value as u16 + carry as u16) > 0xFF;
        let half_carry = (self.registers.a & 0x0F) + (value & 0x0F) + carry > 0x0F;

        self.registers.a = result;
        self.registers.set_flag(registers::Flag::Z, result == 0);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, half_carry);
        self.registers.set_flag(registers::Flag::C, full_carry);
    }

    fn sub_a(&mut self, value: u8) {
        let result = self.registers.a.wrapping_sub(value);
        let borrow = (self.registers.a as u16) < (value as u16);
        let half_borrow = (self.registers.a & 0x0F) < (value & 0x0F);

        self.registers.a = result;
        self.registers.set_flag(registers::Flag::Z, result == 0);
        self.registers.set_flag(registers::Flag::N, true);
        self.registers.set_flag(registers::Flag::H, half_borrow);
        self.registers.set_flag(registers::Flag::C, borrow);
    }

    fn and_a(&mut self, value: u8) {
        self.registers.a &= value;
        self.registers.set_flag(registers::Flag::Z, self.registers.a == 0);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, true);
        self.registers.set_flag(registers::Flag::C, false);
    }

    fn sbc_a(&mut self, value: u8) {
        let carry = if self.registers.get_flag(registers::Flag::C) { 1 } else { 0 };
        let sub = (value as u16) + (carry as u16);
        let result = self.registers.a.wrapping_sub(sub as u8);
        let borrow = (self.registers.a as u16) < sub;
        let half_borrow = (self.registers.a & 0x0F) < ((value & 0x0F) + (carry as u8));
        self.registers.a = result;
        self.registers.set_flag(registers::Flag::Z, result == 0);
        self.registers.set_flag(registers::Flag::N, true);
        self.registers.set_flag(registers::Flag::H, half_borrow);
        self.registers.set_flag(registers::Flag::C, borrow);
    }

    fn xor_a(&mut self, value: u8) {
        self.registers.a ^= value;
        self.registers.set_flag(registers::Flag::Z, self.registers.a == 0);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, false);
        self.registers.set_flag(registers::Flag::C, false);
    }

    fn or_a(&mut self, value: u8) {
        self.registers.a |= value;
        self.registers.set_flag(registers::Flag::Z, self.registers.a == 0);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, false);
        self.registers.set_flag(registers::Flag::C, false);
    }

    fn cp_a(&mut self, value: u8) {
        let result = self.registers.a.wrapping_sub(value);
        let borrow = (self.registers.a as u16) < (value as u16);
        let half_borrow = (self.registers.a & 0x0F) < (value & 0x0F);
        
        self.registers.set_flag(registers::Flag::Z, result == 0);
        self.registers.set_flag(registers::Flag::N, true);
        self.registers.set_flag(registers::Flag::H, half_borrow);
        self.registers.set_flag(registers::Flag::C, borrow);
    }

    // Jump operations
    fn jp_cond(&mut self, condition: bool) -> u8 {
        let addr = self.fetch_word();
        if condition {
            self.registers.pc = addr;
            16
        } else {
            12
        }
    }

    fn call(&mut self) -> u8 {
        let addr = self.fetch_word();
        self.push(self.registers.pc);
        self.registers.pc = addr;
        24
    }

    fn call_cond(&mut self, condition: bool) -> u8 {
        let addr = self.fetch_word();
        if condition {
            self.push(self.registers.pc);
            self.registers.pc = addr;
            24
        } else {
            12
        }
    }

    fn ret(&mut self) -> u8 {
        self.registers.pc = self.pop();
        16
    }

    fn ret_cond(&mut self, condition: bool) -> u8 {
        if condition {
            self.registers.pc = self.pop();
            20
        } else {
            8
        }
    }

    // Stack operations
    fn push(&mut self, value: u16) {
        self.registers.sp = self.registers.sp.wrapping_sub(1);
        self.memory.write_byte(self.registers.sp, (value >> 8) as u8);
        self.registers.sp = self.registers.sp.wrapping_sub(1);
        self.memory.write_byte(self.registers.sp, (value & 0xFF) as u8);
    }

    fn pop(&mut self) -> u16 {
        let low = self.memory.read_byte(self.registers.sp) as u16;
        self.registers.sp = self.registers.sp.wrapping_add(1);
        let high = self.memory.read_byte(self.registers.sp) as u16;
        self.registers.sp = self.registers.sp.wrapping_add(1);
        (high << 8) | low
    }

    // Rotate and shift operations
    fn rlca(&mut self) {
        let carry = (self.registers.a & 0x80) >> 7;
        self.registers.a = (self.registers.a << 1) | carry;
        self.registers.set_flag(registers::Flag::C, carry == 1);
        self.registers.set_flag(registers::Flag::Z, false);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, false);
    }

    fn rrca(&mut self) {
        let carry = self.registers.a & 0x01;
        self.registers.a = (self.registers.a >> 1) | (carry << 7);
        self.registers.set_flag(registers::Flag::C, carry == 1);
        self.registers.set_flag(registers::Flag::Z, false);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, false);
    }

    fn rla(&mut self) {
        let old_carry = if self.registers.get_flag(registers::Flag::C) { 1 } else { 0 };
        let new_carry = (self.registers.a & 0x80) >> 7;
        self.registers.a = (self.registers.a << 1) | old_carry;
        self.registers.set_flag(registers::Flag::C, new_carry == 1);
        self.registers.set_flag(registers::Flag::Z, false);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, false);
    }

    fn rra(&mut self) {
        let old_carry = if self.registers.get_flag(registers::Flag::C) { 1 } else { 0 };
        let new_carry = self.registers.a & 0x01;
        self.registers.a = (self.registers.a >> 1) | (old_carry << 7);
        self.registers.set_flag(registers::Flag::C, new_carry == 1);
        self.registers.set_flag(registers::Flag::Z, false);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, false);
    }

    // CB prefix rotate operations
    fn rlc(&mut self, value: u8) -> u8 {
        let carry = (value & 0x80) >> 7;
        let result = (value << 1) | carry;
        self.registers.set_flag(registers::Flag::Z, result == 0);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, false);
        self.registers.set_flag(registers::Flag::C, carry == 1);
        result
    }

    fn rrc(&mut self, value: u8) -> u8 {
        let carry = value & 0x01;
        let result = (value >> 1) | (carry << 7);
        self.registers.set_flag(registers::Flag::Z, result == 0);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, false);
        self.registers.set_flag(registers::Flag::C, carry == 1);
        result
    }

    fn rl(&mut self, value: u8) -> u8 {
        let old_carry = if self.registers.get_flag(registers::Flag::C) { 1 } else { 0 };
        let new_carry = (value & 0x80) >> 7;
        let result = (value << 1) | old_carry;
        self.registers.set_flag(registers::Flag::Z, result == 0);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, false);
        self.registers.set_flag(registers::Flag::C, new_carry == 1);
        result
    }

    fn rr(&mut self, value: u8) -> u8 {
        let old_carry = if self.registers.get_flag(registers::Flag::C) { 1 } else { 0 };
        let new_carry = value & 0x01;
        let result = (value >> 1) | (old_carry << 7);
        self.registers.set_flag(registers::Flag::Z, result == 0);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, false);
        self.registers.set_flag(registers::Flag::C, new_carry == 1);
        result
    }

    fn sla(&mut self, value: u8) -> u8 {
        let carry = (value & 0x80) >> 7;
        let result = value << 1;
        self.registers.set_flag(registers::Flag::Z, result == 0);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, false);
        self.registers.set_flag(registers::Flag::C, carry == 1);
        result
    }

    fn sra(&mut self, value: u8) -> u8 {
        let carry = value & 0x01;
        let result = (value >> 1) | (value & 0x80);
        self.registers.set_flag(registers::Flag::Z, result == 0);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, false);
        self.registers.set_flag(registers::Flag::C, carry == 1);
        result
    }

    fn swap(&mut self, value: u8) -> u8 {
        let result = ((value & 0x0F) << 4) | ((value & 0xF0) >> 4);
        self.registers.set_flag(registers::Flag::Z, result == 0);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, false);
        self.registers.set_flag(registers::Flag::C, false);
        result
    }

    fn add_hl(&mut self, value: u16) {
        let hl = self.registers.get_hl();
        let result = hl.wrapping_add(value);
        let half = ((hl & 0x0FFF) as u32 + (value & 0x0FFF) as u32) > 0x0FFF;
        let carry = (hl as u32 + value as u32) > 0xFFFF;
        self.registers.set_hl(result);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, half);
        self.registers.set_flag(registers::Flag::C, carry);
    }
    // Decimal adjust accumulator - fairly nuanced; implement standard BCD adjust
    fn daa(&mut self) {
        let mut a = self.registers.a;
        let mut adjust: u8 = 0;
        let mut carry = self.registers.get_flag(registers::Flag::C);

        if self.registers.get_flag(registers::Flag::H) || (!self.registers.get_flag(registers::Flag::N) && (a & 0x0F) > 9) {
            adjust |= 0x06;
        }
        if carry || (!self.registers.get_flag(registers::Flag::N) && a > 0x99) {
            adjust |= 0x60;
            carry = true;
        }

        if self.registers.get_flag(registers::Flag::N) {
            a = a.wrapping_sub(adjust);
        } else {
            a = a.wrapping_add(adjust);
        }

        self.registers.a = a;
        self.registers.set_flag(registers::Flag::Z, a == 0);
        self.registers.set_flag(registers::Flag::H, false);
        self.registers.set_flag(registers::Flag::C, carry);
    }

    fn cpl(&mut self) {
        self.registers.a = !self.registers.a;
        self.registers.set_flag(registers::Flag::N, true);
        self.registers.set_flag(registers::Flag::H, true);
    }

    fn scf(&mut self) {
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, false);
        self.registers.set_flag(registers::Flag::C, true);
    }

    fn ccf(&mut self) {
        let old_c = self.registers.get_flag(registers::Flag::C);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, old_c);
        self.registers.set_flag(registers::Flag::C, !old_c);
    }

    fn srl(&mut self, value: u8) -> u8 {
        let carry = value & 0x01;
        let result = value >> 1;
        self.registers.set_flag(registers::Flag::Z, result == 0);
        self.registers.set_flag(registers::Flag::N, false);
        self.registers.set_flag(registers::Flag::H, false);
        self.registers.set_flag(registers::Flag::C, carry == 1);
        result
    }

    pub fn load_rom(&mut self, rom_data: &[u8]) {
        self.memory.load_rom(rom_data);
    }
}
