pub mod registers;
pub mod instructions;

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

            // SUB r8
            0x90..=0x97 => {
                let src = opcode & 0x07;
                let value = self.get_r8(src);
                self.sub_a(value);
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

            // Jumps and calls
            0xC3 => { self.registers.pc = self.fetch_word(); 16 }    // JP nn
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

            // Immediate arithmetic
            0xC6 => { let val = self.fetch_byte(); self.add_a(val); 8 }  // ADD A, n
            0xCE => { let val = self.fetch_byte(); self.adc_a(val); 8 }  // ADC A, n
            0xD6 => { let val = self.fetch_byte(); self.sub_a(val); 8 }  // SUB n
            0xE6 => { let val = self.fetch_byte(); self.and_a(val); 8 }  // AND n
            0xEE => { let val = self.fetch_byte(); self.xor_a(val); 8 }  // XOR n
            0xF6 => { let val = self.fetch_byte(); self.or_a(val); 8 }   // OR n
            0xFE => { let val = self.fetch_byte(); self.cp_a(val); 8 }   // CP n

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

            // Misc
            0x00 => 4,    // NOP
            0x76 => { self.halted = true; 4 } // HALT

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

// Memory Management Unit, TODO: move to mmu.rs file
pub struct Memory {
    // Memory regions
    rom_bank_0: [u8; 0x4000],    // 0x0000-0x3FFF - ROM Bank 0 (fixed)
    rom_bank_n: [u8; 0x4000],    // 0x4000-0x7FFF - ROM Bank 1-N (switchable)
    vram: [u8; 0x2000],          // 0x8000-0x9FFF - Video RAM
    external_ram: [u8; 0x2000],  // 0xA000-0xBFFF - External RAM
    wram: [u8; 0x2000],          // 0xC000-0xDFFF - Work RAM
    echo_ram: [u8; 0x1E00],      // 0xE000-0xFDFF - Echo of Work RAM
    oam: [u8; 0xA0],             // 0xFE00-0xFE9F - Object Attribute Memory
    unusable: [u8; 0x60],        // 0xFEA0-0xFEFF - Unusable
    io_registers: [u8; 0x80],    // 0xFF00-0xFF7F - I/O Registers
    hram: [u8; 0x7F],            // 0xFF80-0xFFFE - High RAM
    interrupt_enable: u8,        // 0xFFFF - Interrupt Enable Register
    
    // Memory bank controller state
    mbc_type: MBCType,
    rom_bank: usize,
    ram_bank: usize,
    ram_enabled: bool,
    banking_mode: u8,
}

#[derive(Debug, Clone, Copy)]
pub enum MBCType {
    None,
    MBC1,
    MBC2,
    MBC3,
    MBC5,
}

impl Memory {
    pub fn new() -> Self {
        Memory {
            rom_bank_0: [0; 0x4000],
            rom_bank_n: [0; 0x4000],
            vram: [0; 0x2000],
            external_ram: [0; 0x2000],
            wram: [0; 0x2000],
            echo_ram: [0; 0x1E00],
            oam: [0; 0xA0],
            unusable: [0; 0x60],
            io_registers: [0; 0x80],
            hram: [0; 0x7F],
            interrupt_enable: 0,
            mbc_type: MBCType::None,
            rom_bank: 1,
            ram_bank: 0,
            ram_enabled: false,
            banking_mode: 0,
        }
    }

    pub fn read_byte(&self, addr: u16) -> u8 {
        match addr {
            // ROM Bank 0
            0x0000..=0x3FFF => self.rom_bank_0[addr as usize],
            
            // ROM Bank 1-N (switchable)
            0x4000..=0x7FFF => self.rom_bank_n[(addr - 0x4000) as usize],
            
            // Video RAM
            0x8000..=0x9FFF => self.vram[(addr - 0x8000) as usize],
            
            // External RAM (cartridge RAM)
            0xA000..=0xBFFF => {
                if self.ram_enabled {
                    self.external_ram[(addr - 0xA000) as usize]
                } else {
                    0xFF
                }
            }
            
            // Work RAM
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize],
            
            // Echo of Work RAM
            0xE000..=0xFDFF => self.wram[(addr - 0xE000) as usize],
            
            // Object Attribute Memory (OAM)
            0xFE00..=0xFE9F => self.oam[(addr - 0xFE00) as usize],
            
            // Unusable memory area
            0xFEA0..=0xFEFF => 0xFF,
            
            // I/O Registers
            0xFF00..=0xFF7F => self.read_io_register(addr),
            
            // High RAM
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],
            
            // Interrupt Enable Register
            0xFFFF => self.interrupt_enable,
        }
    }

    pub fn write_byte(&mut self, addr: u16, value: u8) {
        match addr {
            // ROM area - MBC register writes
            0x0000..=0x7FFF => self.write_mbc_register(addr, value),
            
            // Video RAM
            0x8000..=0x9FFF => self.vram[(addr - 0x8000) as usize] = value,
            
            // External RAM
            0xA000..=0xBFFF => {
                if self.ram_enabled {
                    self.external_ram[(addr - 0xA000) as usize] = value;
                }
            }
            
            // Work RAM
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize] = value,
            
            // Echo of Work RAM
            0xE000..=0xFDFF => self.wram[(addr - 0xE000) as usize] = value,
            
            // Object Attribute Memory (OAM)
            0xFE00..=0xFE9F => self.oam[(addr - 0xFE00) as usize] = value,
            
            // Unusable memory area
            0xFEA0..=0xFEFF => {} // Ignore writes
            
            // I/O Registers
            0xFF00..=0xFF7F => self.write_io_register(addr, value),
            
            // High RAM
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = value,
            
            // Interrupt Enable Register
            0xFFFF => self.interrupt_enable = value,
        }
    }

    fn read_io_register(&self, addr: u16) -> u8 {
        match addr {
            // Joypad
            0xFF00 => self.io_registers[0x00],
            
            // Serial transfer
            0xFF01..=0xFF02 => self.io_registers[(addr - 0xFF00) as usize],
            
            // Timer
            0xFF04..=0xFF07 => self.io_registers[(addr - 0xFF00) as usize],
            
            // Interrupt Flag
            0xFF0F => self.io_registers[0x0F],
            
            // Sound registers
            0xFF10..=0xFF3F => self.io_registers[(addr - 0xFF00) as usize],
            
            // LCD registers
            0xFF40..=0xFF4B => self.io_registers[(addr - 0xFF00) as usize],
            
            // DMA register
            0xFF46 => self.io_registers[0x46],
            
            // Boot ROM disable
            0xFF50 => self.io_registers[0x50],
            
            _ => self.io_registers[(addr - 0xFF00) as usize],
        }
    }

    fn write_io_register(&mut self, addr: u16, value: u8) {
        match addr {
            // DMA transfer
            0xFF46 => {
                self.io_registers[0x46] = value;
                self.dma_transfer(value);
            }
            
            // All other I/O registers
            _ => self.io_registers[(addr - 0xFF00) as usize] = value,
        }
    }

    fn write_mbc_register(&mut self, addr: u16, value: u8) {
        match self.mbc_type {
            MBCType::None => {} // No banking
            
            MBCType::MBC1 => {
                match addr {
                    0x0000..=0x1FFF => {
                        // RAM Enable
                        self.ram_enabled = (value & 0x0F) == 0x0A;
                    }
                    0x2000..=0x3FFF => {
                        // ROM Bank Number (lower 5 bits)
                        let mut bank = (value & 0x1F) as usize;
                        if bank == 0 { bank = 1; } // Bank 0 maps to 1
                        self.rom_bank = (self.rom_bank & 0x60) | bank;
                    }
                    0x4000..=0x5FFF => {
                        // RAM Bank Number or ROM Bank Number (upper 2 bits)
                        let upper_bits = ((value & 0x03) as usize) << 5;
                        if self.banking_mode == 0 {
                            // ROM banking mode
                            self.rom_bank = (self.rom_bank & 0x1F) | upper_bits;
                        } else {
                            // RAM banking mode
                            self.ram_bank = (value & 0x03) as usize;
                        }
                    }
                    0x6000..=0x7FFF => {
                        // Banking Mode Select
                        self.banking_mode = value & 0x01;
                    }
                    _ => {}
                }
            }
            
            _ => {} // Other MBC types not implemented yet
        }
    }

    fn dma_transfer(&mut self, source: u8) {
        let source_addr = (source as u16) << 8;
        for i in 0..0xA0 {
            let byte = self.read_byte(source_addr + i);
            self.oam[i as usize] = byte;
        }
    }

    pub fn load_rom(&mut self, rom_data: &[u8]) {
        // Determine MBC type from cartridge header
        if rom_data.len() > 0x147 {
            self.mbc_type = match rom_data[0x147] {
                0x00 => MBCType::None,
                0x01..=0x03 => MBCType::MBC1,
                0x05..=0x06 => MBCType::MBC2,
                0x0F..=0x13 => MBCType::MBC3,
                0x19..=0x1E => MBCType::MBC5,
                _ => MBCType::None,
            };
        }

        // Load ROM Bank 0
        let bank_0_size = std::cmp::min(rom_data.len(), 0x4000);
        self.rom_bank_0[..bank_0_size].copy_from_slice(&rom_data[..bank_0_size]);

        // Load ROM Bank 1 (if available)
        if rom_data.len() > 0x4000 {
            let bank_1_size = std::cmp::min(rom_data.len() - 0x4000, 0x4000);
            self.rom_bank_n[..bank_1_size].copy_from_slice(&rom_data[0x4000..0x4000 + bank_1_size]);
        }
    }
}