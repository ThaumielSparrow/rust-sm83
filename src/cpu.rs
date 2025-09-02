use crate::mbc;
use crate::mmu::MMU;
use crate::register::CpuFlag::{C, H, N, Z};
use crate::register::Registers;
use crate::StrResult;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct CPU {
    reg: Registers,
    pub mmu: MMU,
    halted: bool,
    halt_bug: bool,
    ime: bool,
    setdi: u32,
    setei: u32,
}

impl CPU {
    pub fn new(
        cart: Box<dyn mbc::MBC + 'static>,
        _serial_callback: Option<Box<()>>,
    ) -> StrResult<CPU> {
        let cpu_mmu = MMU::new(cart, None)?;
        let registers = Registers::new(cpu_mmu.gbmode);
        Ok(CPU {
            reg: registers,
            halted: false,
            halt_bug: false,
            ime: true,
            setdi: 0,
            setei: 0,
            mmu: cpu_mmu,
        })
    }

    pub fn new_cgb(
        cart: Box<dyn mbc::MBC + 'static>,
        _serial_callback: Option<Box<()>>,
    ) -> StrResult<CPU> {
        let cpu_mmu = MMU::new_cgb(cart, None)?;
        let registers = Registers::new(cpu_mmu.gbmode);
        Ok(CPU {
            reg: registers,
            halted: false,
            halt_bug: false,
            ime: true,
            setdi: 0,
            setei: 0,
            mmu: cpu_mmu,
        })
    }

    pub fn do_cycle(&mut self) -> u32 {
        let ticks = self.docycle() * 4;
        return self.mmu.do_cycle(ticks);
    }

    fn docycle(&mut self) -> u32 {
        self.updateime();
        match self.handleinterrupt() {
            0 => {}
            n => return n,
        };

        if self.halted {
            // Emulate a noop instruction
            1
        } else {
            self.call()
        }
    }

    fn fetchbyte(&mut self) -> u8 {
        let b = self.mmu.rb(self.reg.pc);
        if self.halt_bug {
            self.halt_bug = false;
        } else {
            self.reg.pc = self.reg.pc.wrapping_add(1);
        }
        b
    }

    fn fetchword(&mut self) -> u16 {
        let w = self.mmu.rw(self.reg.pc);
        self.reg.pc += 2;
        w
    }

    fn updateime(&mut self) {
        self.setdi = match self.setdi {
            2 => 1,
            1 => {
                self.ime = false;
                0
            }
            _ => 0,
        };
        self.setei = match self.setei {
            2 => 1,
            1 => {
                self.ime = true;
                0
            }
            _ => 0,
        };
    }

    fn handleinterrupt(&mut self) -> u32 {
        if self.ime == false && self.halted == false {
            return 0;
        }

        let triggered = self.mmu.inte & self.mmu.intf & 0x1F;
        if triggered == 0 {
            return 0;
        }

        self.halted = false;
        if self.ime == false {
            return 0;
        }
        self.ime = false;

        let n = triggered.trailing_zeros();
        if n >= 5 {
            panic!("Invalid interrupt triggered");
        }
        self.mmu.intf &= !(1 << n);
        let pc = self.reg.pc;
        self.pushstack(pc);
        self.reg.pc = 0x0040 | ((n as u16) << 3);

        4
    }

    fn pushstack(&mut self, value: u16) {
        self.reg.sp = self.reg.sp.wrapping_sub(2);
        self.mmu.ww(self.reg.sp, value);
    }

    fn popstack(&mut self) -> u16 {
        let res = self.mmu.rw(self.reg.sp);
        self.reg.sp += 2;
        res
    }

    // Table based dispatcher
    fn call(&mut self) -> u32 {
        let opcode = self.fetchbyte();
        OPCODE_TABLE[opcode as usize](self, opcode)
    }

    fn alu_add(&mut self, b: u8, usec: bool) {
        let c = if usec && self.reg.getflag(C) { 1 } else { 0 };
        let a = self.reg.a;
        let r = a.wrapping_add(b).wrapping_add(c);
        self.reg.flag(Z, r == 0);
        self.reg.flag(H, (a & 0xF) + (b & 0xF) + c > 0xF);
        self.reg.flag(N, false);
        self.reg
            .flag(C, (a as u16) + (b as u16) + (c as u16) > 0xFF);
        self.reg.a = r;
    }

    fn alu_sub(&mut self, b: u8, usec: bool) {
        let c = if usec && self.reg.getflag(C) { 1 } else { 0 };
        let a = self.reg.a;
        let r = a.wrapping_sub(b).wrapping_sub(c);
        self.reg.flag(Z, r == 0);
        self.reg.flag(H, (a & 0x0F) < (b & 0x0F) + c);
        self.reg.flag(N, true);
        self.reg.flag(C, (a as u16) < (b as u16) + (c as u16));
        self.reg.a = r;
    }

    fn alu_and(&mut self, b: u8) {
        let r = self.reg.a & b;
        self.reg.flag(Z, r == 0);
        self.reg.flag(H, true);
        self.reg.flag(C, false);
        self.reg.flag(N, false);
        self.reg.a = r;
    }

    fn alu_inc(&mut self, a: u8) -> u8 {
        let r = a.wrapping_add(1);
        self.reg.flag(Z, r == 0);
        self.reg.flag(H, (a & 0x0F) + 1 > 0x0F);
        self.reg.flag(N, false);
        return r;
    }

    fn alu_dec(&mut self, a: u8) -> u8 {
        let r = a.wrapping_sub(1);
        self.reg.flag(Z, r == 0);
        self.reg.flag(H, (a & 0x0F) == 0);
        self.reg.flag(N, true);
        return r;
    }

    fn alu_add16(&mut self, b: u16) {
        let a = self.reg.hl();
        let r = a.wrapping_add(b);
        self.reg.flag(H, (a & 0x0FFF) + (b & 0x0FFF) > 0x0FFF);
        self.reg.flag(N, false);
        self.reg.flag(C, a > 0xFFFF - b);
        self.reg.sethl(r);
    }

    fn alu_add16imm(&mut self, a: u16) -> u16 {
        let b = self.fetchbyte() as i8 as i16 as u16;
        self.reg.flag(N, false);
        self.reg.flag(Z, false);
        self.reg.flag(H, (a & 0x000F) + (b & 0x000F) > 0x000F);
        self.reg.flag(C, (a & 0x00FF) + (b & 0x00FF) > 0x00FF);
        return a.wrapping_add(b);
    }

    fn alu_swap(&mut self, a: u8) -> u8 {
        self.reg.flag(Z, a == 0);
        self.reg.flag(C, false);
        self.reg.flag(H, false);
        self.reg.flag(N, false);
        (a >> 4) | (a << 4)
    }

    fn alu_srflagupdate(&mut self, r: u8, c: bool) {
        self.reg.flag(H, false);
        self.reg.flag(N, false);
        self.reg.flag(Z, r == 0);
        self.reg.flag(C, c);
    }

    fn alu_rlc(&mut self, a: u8) -> u8 {
        let c = a & 0x80 == 0x80;
        let r = (a << 1) | (if c { 1 } else { 0 });
        self.alu_srflagupdate(r, c);
        return r;
    }

    fn alu_rl(&mut self, a: u8) -> u8 {
        let c = a & 0x80 == 0x80;
        let r = (a << 1) | (if self.reg.getflag(C) { 1 } else { 0 });
        self.alu_srflagupdate(r, c);
        return r;
    }

    fn alu_rrc(&mut self, a: u8) -> u8 {
        let c = a & 0x01 == 0x01;
        let r = (a >> 1) | (if c { 0x80 } else { 0 });
        self.alu_srflagupdate(r, c);
        return r;
    }

    fn alu_rr(&mut self, a: u8) -> u8 {
        let c = a & 0x01 == 0x01;
        let r = (a >> 1) | (if self.reg.getflag(C) { 0x80 } else { 0 });
        self.alu_srflagupdate(r, c);
        return r;
    }

    fn alu_sla(&mut self, a: u8) -> u8 {
        let c = a & 0x80 == 0x80;
        let r = a << 1;
        self.alu_srflagupdate(r, c);
        return r;
    }

    fn alu_sra(&mut self, a: u8) -> u8 {
        let c = a & 0x01 == 0x01;
        let r = (a >> 1) | (a & 0x80);
        self.alu_srflagupdate(r, c);
        return r;
    }

    fn alu_srl(&mut self, a: u8) -> u8 {
        let c = a & 0x01 == 0x01;
        let r = a >> 1;
        self.alu_srflagupdate(r, c);
        return r;
    }

    fn alu_bit(&mut self, a: u8, b: u8) {
        let r = a & (1 << (b as u32)) == 0;
        self.reg.flag(N, false);
        self.reg.flag(H, true);
        self.reg.flag(Z, r);
    }

    fn alu_daa(&mut self) {
        let mut a = self.reg.a;
        let mut adjust = if self.reg.getflag(C) { 0x60 } else { 0x00 };
        if self.reg.getflag(H) {
            adjust |= 0x06;
        };
        if !self.reg.getflag(N) {
            if a & 0x0F > 0x09 {
                adjust |= 0x06;
            };
            if a > 0x99 {
                adjust |= 0x60;
            };
            a = a.wrapping_add(adjust);
        } else {
            a = a.wrapping_sub(adjust);
        }

        self.reg.flag(C, adjust >= 0x60);
        self.reg.flag(H, false);
        self.reg.flag(Z, a == 0);
        self.reg.a = a;
    }

    pub fn read_byte(&mut self, address: u16) -> u8 {
        self.mmu.rb(address)
    }
    pub fn write_byte(&mut self, address: u16, byte: u8) {
        self.mmu.wb(address, byte)
    }

    pub fn read_wide(&mut self, address: u16) -> u16 {
        self.mmu.rw(address)
    }
    pub fn write_wide(&mut self, address: u16, wide: u16) {
        self.mmu.ww(address, wide)
    }
}

// Type alias for opcode handlers: &mut CPU, opcode -> cycles
type OpHandler = fn(&mut CPU, u8) -> u32;

// Plain functions (initial subset) to avoid macro complexity / compile errors.
fn op_00(_cpu: &mut CPU, _op: u8) -> u32 { 1 }
fn op_76(cpu: &mut CPU, _op: u8) -> u32 { cpu.halted = true; cpu.halt_bug = cpu.mmu.intf & cpu.mmu.inte & 0x1F != 0; 1 }

fn op_ld_rr_d16(cpu: &mut CPU, op: u8) -> u32 {
    let val = cpu.fetchword();
    match op { 0x01 => cpu.reg.setbc(val), 0x11 => cpu.reg.setde(val), 0x21 => cpu.reg.sethl(val), 0x31 => cpu.reg.sp = val, _ => unreachable!(), }
    3
}

fn op_inc_r(cpu: &mut CPU, op: u8) -> u32 {
    match op { 0x04 => cpu.reg.b = cpu.alu_inc(cpu.reg.b), 0x0C => cpu.reg.c = cpu.alu_inc(cpu.reg.c), 0x14 => cpu.reg.d = cpu.alu_inc(cpu.reg.d), 0x1C => cpu.reg.e = cpu.alu_inc(cpu.reg.e), 0x24 => cpu.reg.h = cpu.alu_inc(cpu.reg.h), 0x2C => cpu.reg.l = cpu.alu_inc(cpu.reg.l), 0x3C => cpu.reg.a = cpu.alu_inc(cpu.reg.a), _ => unreachable!(), }
    1
}

fn op_dec_r(cpu: &mut CPU, op: u8) -> u32 {
    match op { 0x05 => cpu.reg.b = cpu.alu_dec(cpu.reg.b), 0x0D => cpu.reg.c = cpu.alu_dec(cpu.reg.c), 0x15 => cpu.reg.d = cpu.alu_dec(cpu.reg.d), 0x1D => cpu.reg.e = cpu.alu_dec(cpu.reg.e), 0x25 => cpu.reg.h = cpu.alu_dec(cpu.reg.h), 0x2D => cpu.reg.l = cpu.alu_dec(cpu.reg.l), 0x3D => cpu.reg.a = cpu.alu_dec(cpu.reg.a), _ => unreachable!(), }
    1
}

fn op_ld_r_d8(cpu: &mut CPU, op: u8) -> u32 {
    let v = cpu.fetchbyte();
    match op { 0x06 => cpu.reg.b = v, 0x0E => cpu.reg.c = v, 0x16 => cpu.reg.d = v, 0x1E => cpu.reg.e = v, 0x26 => cpu.reg.h = v, 0x2E => cpu.reg.l = v, 0x3E => cpu.reg.a = v, _ => unreachable!(), }
    2
}

fn op_rot_a(cpu: &mut CPU, op: u8) -> u32 {
    match op { 0x07 => { cpu.reg.a = cpu.alu_rlc(cpu.reg.a); }, 0x0F => { cpu.reg.a = cpu.alu_rrc(cpu.reg.a); }, 0x17 => { cpu.reg.a = cpu.alu_rl(cpu.reg.a); }, 0x1F => { cpu.reg.a = cpu.alu_rr(cpu.reg.a); }, _ => unreachable!(), }
    cpu.reg.flag(Z, false); 1
}

// LD r,r' (and LD r,(HL)) group 0x40-0x7F excluding 0x76 HALT already mapped.
fn op_ld_r_r(cpu: &mut CPU, op: u8) -> u32 {
    let hl = cpu.reg.hl();
    let read_hl = |cpu: &mut CPU| cpu.mmu.rb(hl);
    // decode dest/source indices: bits 3-5 dest, 0-2 src (A=7,B=0,C=1,D=2,E=3,H=4,L=5,(HL)=6)
    let dest = (op >> 3) & 0x07; let src = op & 0x07;
    let val = match src { 0 => cpu.reg.b, 1 => cpu.reg.c, 2 => cpu.reg.d, 3 => cpu.reg.e, 4 => cpu.reg.h, 5 => cpu.reg.l, 6 => read_hl(cpu), 7 => cpu.reg.a, _ => unreachable!() };
    match dest { 0 => cpu.reg.b = val, 1 => cpu.reg.c = val, 2 => cpu.reg.d = val, 3 => cpu.reg.e = val, 4 => cpu.reg.h = val, 5 => cpu.reg.l = val, 6 => { // LD (HL),r
            cpu.mmu.wb(hl, val);
        }, 7 => cpu.reg.a = val, _ => unreachable!() };
    if dest == 6 { 2 } else { 1 }
}

// LD r,d8 already handled for specific opcodes; add LD (HL),d8 (0x36)
fn op_ld_hl_d8(cpu: &mut CPU, _op: u8) -> u32 { let v = cpu.fetchbyte(); let hl = cpu.reg.hl(); cpu.mmu.wb(hl, v); 3 }

// ALU helpers mapping register code to value (including (HL))
fn read_reg_by_code(cpu: &mut CPU, code: u8) -> u8 { match code { 0 => cpu.reg.b, 1 => cpu.reg.c, 2 => cpu.reg.d, 3 => cpu.reg.e, 4 => cpu.reg.h, 5 => cpu.reg.l, 6 => cpu.mmu.rb(cpu.reg.hl()), 7 => cpu.reg.a, _ => unreachable!() } }

// ADD/ADC/SUB/SBC/AND/XOR/OR/CP r (0x80-0xBF)
fn op_alu_r(cpu: &mut CPU, op: u8) -> u32 {
    let opclass = (op >> 3) & 0x07; // which alu op
    let src = op & 0x07;
    let v = read_reg_by_code(cpu, src);
    match opclass {
        0 => { cpu.alu_add(v, false); }, // ADD
        1 => { cpu.alu_add(v, true); },  // ADC
        2 => { cpu.alu_sub(v, false); }, // SUB
        3 => { cpu.alu_sub(v, true); },  // SBC
        4 => { cpu.alu_and(v); },        // AND
        5 => { cpu.reg.a ^= v; cpu.reg.flag(Z, cpu.reg.a == 0); cpu.reg.flag(N, false); cpu.reg.flag(H, false); cpu.reg.flag(C, false); }, // XOR
        6 => { // OR
            cpu.reg.a |= v; cpu.reg.flag(Z, cpu.reg.a == 0); cpu.reg.flag(N, false); cpu.reg.flag(H, false); cpu.reg.flag(C, false);
        },
        7 => { // CP (compare a - v)
            let a = cpu.reg.a; let r = a.wrapping_sub(v); cpu.reg.flag(Z, r == 0); cpu.reg.flag(N, true); cpu.reg.flag(H, (a & 0x0F) < (v & 0x0F)); cpu.reg.flag(C, a < v);
        },
        _ => unreachable!()
    }
    if src == 6 { 2 } else { 1 }
}

// Immediate variants: ADD/ADC/SUB/SBC/AND/XOR/OR/CP d8 (0xC6,0xCE,0xD6,0xDE,0xE6,0xEE,0xF6,0xFE)
fn op_alu_d8(cpu: &mut CPU, op: u8) -> u32 {
    let v = cpu.fetchbyte();
    match op {
        0xC6 => cpu.alu_add(v, false),
        0xCE => cpu.alu_add(v, true),
        0xD6 => cpu.alu_sub(v, false),
        0xDE => cpu.alu_sub(v, true),
        0xE6 => cpu.alu_and(v),
        0xEE => { cpu.reg.a ^= v; cpu.reg.flag(Z, cpu.reg.a == 0); cpu.reg.flag(N, false); cpu.reg.flag(H, false); cpu.reg.flag(C, false); },
        0xF6 => { cpu.reg.a |= v; cpu.reg.flag(Z, cpu.reg.a == 0); cpu.reg.flag(N, false); cpu.reg.flag(H, false); cpu.reg.flag(C, false); },
        0xFE => { let a = cpu.reg.a; let r = a.wrapping_sub(v); cpu.reg.flag(Z, r == 0); cpu.reg.flag(N, true); cpu.reg.flag(H, (a & 0x0F) < (v & 0x0F)); cpu.reg.flag(C, a < v); },
        _ => unreachable!(),
    }
    2
}

// INC rr (0x03,13,23,33)
fn op_inc_rr(cpu: &mut CPU, op: u8) -> u32 {
    match op {
        0x03 => cpu.reg.setbc(cpu.reg.bc().wrapping_add(1)),
        0x13 => cpu.reg.setde(cpu.reg.de().wrapping_add(1)),
        0x23 => { let v = cpu.reg.hl().wrapping_add(1); cpu.reg.sethl(v); },
        0x33 => cpu.reg.sp = cpu.reg.sp.wrapping_add(1),
        _ => unreachable!(),
    }
    2
}

// DEC rr (0x0B,1B,2B,3B)
fn op_dec_rr(cpu: &mut CPU, op: u8) -> u32 {
    match op {
        0x0B => cpu.reg.setbc(cpu.reg.bc().wrapping_sub(1)),
        0x1B => cpu.reg.setde(cpu.reg.de().wrapping_sub(1)),
        0x2B => { let v = cpu.reg.hl().wrapping_sub(1); cpu.reg.sethl(v); },
        0x3B => cpu.reg.sp = cpu.reg.sp.wrapping_sub(1),
        _ => unreachable!(),
    }
    2
}

// ADD HL,rr (0x09,19,29,39) cycles:2
fn op_add_hl_rr(cpu: &mut CPU, op: u8) -> u32 {
    match op {
        0x09 => cpu.alu_add16(cpu.reg.bc()),
        0x19 => cpu.alu_add16(cpu.reg.de()),
        0x29 => { let v = cpu.reg.hl(); cpu.alu_add16(v); },
        0x39 => cpu.alu_add16(cpu.reg.sp),
        _ => unreachable!(),
    }
    2
}

// PUSH rr (0xC5,0xD5,0xE5,0xF5) note af on 0xF5
fn op_push_rr(cpu: &mut CPU, op: u8) -> u32 {
    let v = match op { 0xC5 => cpu.reg.bc(), 0xD5 => cpu.reg.de(), 0xE5 => cpu.reg.hl(), 0xF5 => cpu.reg.af(), _ => unreachable!() };
    cpu.reg.sp = cpu.reg.sp.wrapping_sub(1); cpu.mmu.wb(cpu.reg.sp, (v >> 8) as u8);
    cpu.reg.sp = cpu.reg.sp.wrapping_sub(1); cpu.mmu.wb(cpu.reg.sp, v as u8);
    4
}

// POP rr (0xC1,0xD1,0xE1,0xF1) note af on 0xF1 with lower nibble of F always zeroed by legacy code (flags bits)
fn op_pop_rr(cpu: &mut CPU, op: u8) -> u32 {
    let lo = cpu.mmu.rb(cpu.reg.sp); cpu.reg.sp = cpu.reg.sp.wrapping_add(1);
    let hi = cpu.mmu.rb(cpu.reg.sp); cpu.reg.sp = cpu.reg.sp.wrapping_add(1);
    let v = ((hi as u16) << 8) | (lo as u16);
    match op { 0xC1 => cpu.reg.setbc(v), 0xD1 => cpu.reg.setde(v), 0xE1 => cpu.reg.sethl(v), 0xF1 => { cpu.reg.setaf(v & 0xFFF0); }, _ => unreachable!() }
    3
}

// 0x08 LD (nn),SP and 0xFA LD A,(nn) and 0xEA LD (nn),A handled separately
fn op_store_nn_sp(cpu: &mut CPU, _op:u8) -> u32 { let a = cpu.fetchword(); cpu.mmu.ww(a, cpu.reg.sp); 5 }
fn op_ld_a_nn(cpu: &mut CPU, _op:u8) -> u32 { let a = cpu.fetchword(); cpu.reg.a = cpu.mmu.rb(a); 4 }
fn op_ld_nn_a(cpu: &mut CPU, _op:u8) -> u32 { let a = cpu.fetchword(); cpu.mmu.wb(a, cpu.reg.a); 4 }

// LDH (0xE0,0xF0,0xE2,0xF2)
fn op_ldh(cpu: &mut CPU, op: u8) -> u32 {
    match op {
        0xE0 => { let off = cpu.fetchbyte() as u16; cpu.mmu.wb(0xFF00 + off, cpu.reg.a); 3 },
        0xF0 => { let off = cpu.fetchbyte() as u16; cpu.reg.a = cpu.mmu.rb(0xFF00 + off); 3 },
        0xE2 => { cpu.mmu.wb(0xFF00 + cpu.reg.c as u16, cpu.reg.a); 2 },
        0xF2 => { cpu.reg.a = cpu.mmu.rb(0xFF00 + cpu.reg.c as u16); 2 },
        _ => unreachable!(),
    }
}

// LD HL,SP+e (0xF8) and LD SP,HL (0xF9)
fn op_ld_hl_sp_e(cpu: &mut CPU, _op:u8) -> u32 { let e = cpu.fetchbyte() as i8 as i16 as u16; let sp = cpu.reg.sp; let r = sp.wrapping_add(e); cpu.reg.flag(Z,false); cpu.reg.flag(N,false); cpu.reg.flag(H, (sp ^ e ^ r) & 0x10 != 0); cpu.reg.flag(C, (sp ^ e ^ r) & 0x100 != 0); cpu.reg.sethl(r); 3 }
fn op_ld_sp_hl(cpu: &mut CPU, _op:u8) -> u32 { cpu.reg.sp = cpu.reg.hl(); 2 }

// JR e and conditional JR (0x18,20,28,30,38)
fn op_jr(cpu: &mut CPU, op: u8) -> u32 { let off = cpu.fetchbyte() as i8 as i16 as u16; let take = match op { 0x18 => true, 0x20 => !cpu.reg.getflag(Z), 0x28 => cpu.reg.getflag(Z), 0x30 => !cpu.reg.getflag(C), 0x38 => cpu.reg.getflag(C), _ => unreachable!() }; if take { cpu.reg.pc = cpu.reg.pc.wrapping_add(off); 3 } else { 2 } }

// JP nn and conditional JP (0xC3, C2, CA, D2, DA) ; JP (HL) 0xE9
fn op_jp(cpu: &mut CPU, op: u8) -> u32 {
    match op {
        0xE9 => { cpu.reg.pc = cpu.reg.hl(); 1 },
        0xC3 => { let a = cpu.fetchword(); cpu.reg.pc = a; 4 },
        0xC2 | 0xCA | 0xD2 | 0xDA => {
            let a = cpu.fetchword();
            let cond = match op { 0xC2 => !cpu.reg.getflag(Z), 0xCA => cpu.reg.getflag(Z), 0xD2 => !cpu.reg.getflag(C), 0xDA => cpu.reg.getflag(C), _ => unreachable!() };
            if cond { cpu.reg.pc = a; 4 } else { 3 }
        }
        _ => unreachable!(),
    }
}

// CALL nn and conditional (0xCD, C4, CC, D4, DC)
fn op_call(cpu: &mut CPU, op: u8) -> u32 {
    let addr = cpu.fetchword();
    let cond = match op { 0xCD => true, 0xC4 => !cpu.reg.getflag(Z), 0xCC => cpu.reg.getflag(Z), 0xD4 => !cpu.reg.getflag(C), 0xDC => cpu.reg.getflag(C), _ => unreachable!() };
    if cond { let pc = cpu.reg.pc; cpu.reg.sp = cpu.reg.sp.wrapping_sub(1); cpu.mmu.wb(cpu.reg.sp, (pc >> 8) as u8); cpu.reg.sp = cpu.reg.sp.wrapping_sub(1); cpu.mmu.wb(cpu.reg.sp, pc as u8); cpu.reg.pc = addr; 6 } else { 3 }
}

// RET and conditional (0xC9, C0, C8, D0, D8) ; RETI 0xD9
fn op_ret(cpu: &mut CPU, op: u8) -> u32 {
    match op {
        0xC9 => { let lo = cpu.mmu.rb(cpu.reg.sp); cpu.reg.sp = cpu.reg.sp.wrapping_add(1); let hi = cpu.mmu.rb(cpu.reg.sp); cpu.reg.sp = cpu.reg.sp.wrapping_add(1); cpu.reg.pc = ((hi as u16) << 8) | lo as u16; 4 },
    0xD9 => { let lo = cpu.mmu.rb(cpu.reg.sp); cpu.reg.sp = cpu.reg.sp.wrapping_add(1); let hi = cpu.mmu.rb(cpu.reg.sp); cpu.reg.sp = cpu.reg.sp.wrapping_add(1); cpu.reg.pc = ((hi as u16) << 8) | lo as u16; cpu.setei = 1; 4 },
        0xC0 | 0xC8 | 0xD0 | 0xD8 => {
            let cond = match op { 0xC0 => !cpu.reg.getflag(Z), 0xC8 => cpu.reg.getflag(Z), 0xD0 => !cpu.reg.getflag(C), 0xD8 => cpu.reg.getflag(C), _ => unreachable!() };
            if cond { let lo = cpu.mmu.rb(cpu.reg.sp); cpu.reg.sp = cpu.reg.sp.wrapping_add(1); let hi = cpu.mmu.rb(cpu.reg.sp); cpu.reg.sp = cpu.reg.sp.wrapping_add(1); cpu.reg.pc = ((hi as u16) << 8) | lo as u16; 5 } else { 2 }
        }
        _ => unreachable!(),
    }
}

// RST t (0xC7,CF,D7,DF,E7,EF,F7,FF)
fn op_rst(cpu: &mut CPU, op: u8) -> u32 { let target = match op { 0xC7 => 0x00, 0xCF => 0x08, 0xD7 => 0x10, 0xDF => 0x18, 0xE7 => 0x20, 0xEF => 0x28, 0xF7 => 0x30, 0xFF => 0x38, _ => unreachable!() }; let pc = cpu.reg.pc; cpu.reg.sp = cpu.reg.sp.wrapping_sub(1); cpu.mmu.wb(cpu.reg.sp, (pc >> 8) as u8); cpu.reg.sp = cpu.reg.sp.wrapping_sub(1); cpu.mmu.wb(cpu.reg.sp, pc as u8); cpu.reg.pc = target; 4 }

// Misc single opcodes not yet migrated: DAA(0x27), CPL(0x2F), SCF(0x37), CCF(0x3F), DI(0xF3), EI(0xFB), STOP(0x10)
fn op_misc(cpu: &mut CPU, op: u8) -> u32 { match op { 0x27 => { cpu.alu_daa(); 1 }, 0x2F => { cpu.reg.a = !cpu.reg.a; cpu.reg.flag(H,true); cpu.reg.flag(N,true); 1 }, 0x37 => { cpu.reg.flag(C,true); cpu.reg.flag(H,false); cpu.reg.flag(N,false); 1 }, 0x3F => { let c = cpu.reg.getflag(C); cpu.reg.flag(C,!c); cpu.reg.flag(H,false); cpu.reg.flag(N,false); 1 }, 0xF3 => { cpu.ime = false; 1 }, 0xFB => { cpu.setei = 2; 1 }, 0x10 => { cpu.mmu.switch_speed(); 1 }, _ => unreachable!() } }

// Simple LD A,(rr) and LD (rr),A for BC/DE plus HL +/- already partially handled legacy (0x0A,0x1A,0x02,0x12,0x22,0x2A,0x32,0x3A) bring them in
fn op_ld_a_rr_ind(cpu: &mut CPU, op:u8) -> u32 { match op { 0x0A => cpu.reg.a = cpu.mmu.rb(cpu.reg.bc()), 0x1A => cpu.reg.a = cpu.mmu.rb(cpu.reg.de()), _=> unreachable!()}; 2 }
fn op_ld_rr_ind_a(cpu: &mut CPU, op:u8) -> u32 { match op { 0x02 => cpu.mmu.wb(cpu.reg.bc(), cpu.reg.a), 0x12 => cpu.mmu.wb(cpu.reg.de(), cpu.reg.a), _=> unreachable!()}; 2 }
fn op_ld_hl_incdec_a(cpu:&mut CPU, op:u8) -> u32 { match op { 0x22 => { cpu.mmu.wb(cpu.reg.hli(), cpu.reg.a); }, 0x2A => { cpu.reg.a = cpu.mmu.rb(cpu.reg.hli()); }, 0x32 => { cpu.mmu.wb(cpu.reg.hld(), cpu.reg.a); }, 0x3A => { cpu.reg.a = cpu.mmu.rb(cpu.reg.hld()); }, _=> unreachable!()}; 2 }

// INC (HL) 0x34 , DEC (HL) 0x35
fn op_incdec_hl_mem(cpu:&mut CPU, op:u8) -> u32 { let addr = cpu.reg.hl(); let v = cpu.mmu.rb(addr); let v2 = if op==0x34 { cpu.alu_inc(v) } else { cpu.alu_dec(v) }; cpu.mmu.wb(addr, v2); 3 }

// CB prefix table
type CbHandler = fn(&mut CPU, u8) -> u32;
fn cb_rot(cpu:&mut CPU, op:u8)->u32 { let target = op & 0x07; let group = op >> 3; let mut v = match target {0=>cpu.reg.b,1=>cpu.reg.c,2=>cpu.reg.d,3=>cpu.reg.e,4=>cpu.reg.h,5=>cpu.reg.l,6=>cpu.mmu.rb(cpu.reg.hl()),7=>cpu.reg.a,_=>unreachable!()}; v = match group {0=>cpu.alu_rlc(v),1=>cpu.alu_rrc(v),2=>cpu.alu_rl(v),3=>cpu.alu_rr(v),4=>cpu.alu_sla(v),5=>cpu.alu_sra(v),6=>cpu.alu_swap(v),7=>cpu.alu_srl(v),_=>unreachable!()}; if target==6 { cpu.mmu.wb(cpu.reg.hl(), v); 4 } else { match target {0=>cpu.reg.b=v,1=>cpu.reg.c=v,2=>cpu.reg.d=v,3=>cpu.reg.e=v,4=>cpu.reg.h=v,5=>cpu.reg.l=v,7=>cpu.reg.a=v,_=>{} }; 2 } }
fn cb_bit(cpu:&mut CPU, op:u8)->u32 { let bit = (op>>3)&0x07; let target = op & 0x07; let v = if target==6 { cpu.mmu.rb(cpu.reg.hl()) } else { match target {0=>cpu.reg.b,1=>cpu.reg.c,2=>cpu.reg.d,3=>cpu.reg.e,4=>cpu.reg.h,5=>cpu.reg.l,7=>cpu.reg.a,_=>unreachable!()} }; cpu.alu_bit(v, bit); if target==6 {3} else {2} }
fn cb_res(cpu:&mut CPU, op:u8)->u32 { let bit = (op>>3)&0x07; let mask = !(1<<bit); let target = op & 0x07; if target==6 { let addr=cpu.reg.hl(); let mut v=cpu.mmu.rb(addr); v &= mask; cpu.mmu.wb(addr,v); 4 } else { let reg = match target {0=>&mut cpu.reg.b,1=>&mut cpu.reg.c,2=>&mut cpu.reg.d,3=>&mut cpu.reg.e,4=>&mut cpu.reg.h,5=>&mut cpu.reg.l,7=>&mut cpu.reg.a,_=>unreachable!()}; *reg &= mask; 2 } }
fn cb_set(cpu:&mut CPU, op:u8)->u32 { let bit = (op>>3)&0x07; let mask = 1<<bit; let target = op & 0x07; if target==6 { let addr=cpu.reg.hl(); let mut v=cpu.mmu.rb(addr); v |= mask; cpu.mmu.wb(addr,v); 4 } else { let reg = match target {0=>&mut cpu.reg.b,1=>&mut cpu.reg.c,2=>&mut cpu.reg.d,3=>&mut cpu.reg.e,4=>&mut cpu.reg.h,5=>&mut cpu.reg.l,7=>&mut cpu.reg.a,_=>unreachable!()}; *reg |= mask; 2 } }
static CB_TABLE: [CbHandler;256] = { let mut t:[CbHandler;256] = [cb_rot;256]; let mut i=0; while i<256 { t[i]= if i<0x40 { cb_rot } else if i<0x80 { cb_bit } else if i<0xC0 { cb_res } else { cb_set }; i+=1;} t };
fn op_cb(cpu:&mut CPU,_:u8)->u32 { let opc = cpu.fetchbyte(); CB_TABLE[opc as usize](cpu, opc) }

fn op_fallback(_cpu: &mut CPU, op: u8) -> u32 { panic!("Unimplemented opcode {:02X}", op); }

// Build opcode table. Unmigrated opcodes point to fallback.
#[allow(non_upper_case_globals)]
static OPCODE_TABLE: [OpHandler; 256] = {
    let mut table: [OpHandler; 256] = [op_fallback; 256];
    table[0x00] = op_00;
    table[0x76] = op_76;
    // LD rr,d16
    table[0x01] = op_ld_rr_d16; table[0x11] = op_ld_rr_d16; table[0x21] = op_ld_rr_d16; table[0x31] = op_ld_rr_d16;
    // INC r
    table[0x04] = op_inc_r; table[0x0C] = op_inc_r; table[0x14] = op_inc_r; table[0x1C] = op_inc_r; table[0x24] = op_inc_r; table[0x2C] = op_inc_r; table[0x3C] = op_inc_r;
    // DEC r
    table[0x05] = op_dec_r; table[0x0D] = op_dec_r; table[0x15] = op_dec_r; table[0x1D] = op_dec_r; table[0x25] = op_dec_r; table[0x2D] = op_dec_r; table[0x3D] = op_dec_r;
    // LD r,d8
    table[0x06] = op_ld_r_d8; table[0x0E] = op_ld_r_d8; table[0x16] = op_ld_r_d8; table[0x1E] = op_ld_r_d8; table[0x26] = op_ld_r_d8; table[0x2E] = op_ld_r_d8; table[0x3E] = op_ld_r_d8;
    // Rotates on A
    table[0x07] = op_rot_a; table[0x0F] = op_rot_a; table[0x17] = op_rot_a; table[0x1F] = op_rot_a;
    // 16-bit INC/DEC
    table[0x03] = op_inc_rr; table[0x13] = op_inc_rr; table[0x23] = op_inc_rr; table[0x33] = op_inc_rr;
    table[0x0B] = op_dec_rr; table[0x1B] = op_dec_rr; table[0x2B] = op_dec_rr; table[0x3B] = op_dec_rr;
    // ADD HL,rr
    table[0x09] = op_add_hl_rr; table[0x19] = op_add_hl_rr; table[0x29] = op_add_hl_rr; table[0x39] = op_add_hl_rr;
    // LD (HL),d8
    table[0x36] = op_ld_hl_d8;
    // PUSH/POP
    table[0xC5] = op_push_rr; table[0xD5] = op_push_rr; table[0xE5] = op_push_rr; table[0xF5] = op_push_rr;
    table[0xC1] = op_pop_rr; table[0xD1] = op_pop_rr; table[0xE1] = op_pop_rr; table[0xF1] = op_pop_rr;
    // LD (nn),SP ; LD A,(nn) ; LD (nn),A
    table[0x08] = op_store_nn_sp; table[0xFA] = op_ld_a_nn; table[0xEA] = op_ld_nn_a;
    // LDH variants
    table[0xE0] = op_ldh; table[0xF0] = op_ldh; table[0xE2] = op_ldh; table[0xF2] = op_ldh;
    // LD HL,SP+e and LD SP,HL
    table[0xF8] = op_ld_hl_sp_e; table[0xF9] = op_ld_sp_hl;
    // LD r,r' block (0x40-0x7F except 0x76 HALT) -> one handler
    let mut op = 0x40; while op <= 0x7F { if op != 0x76 { table[op as usize] = op_ld_r_r; } op += 1; }
    // ALU r operations (0x80-0xBF)
    op = 0x80; while op <= 0xBF { table[op as usize] = op_alu_r; op += 1; }
    // ALU immediate ops
    table[0xC6] = op_alu_d8; table[0xCE] = op_alu_d8; table[0xD6] = op_alu_d8; table[0xDE] = op_alu_d8;
    table[0xE6] = op_alu_d8; table[0xEE] = op_alu_d8; table[0xF6] = op_alu_d8; table[0xFE] = op_alu_d8;
    // JR family
    table[0x18] = op_jr; table[0x20] = op_jr; table[0x28] = op_jr; table[0x30] = op_jr; table[0x38] = op_jr;
    // JP family inc JP (HL)
    table[0xC3] = op_jp; table[0xC2] = op_jp; table[0xCA] = op_jp; table[0xD2] = op_jp; table[0xDA] = op_jp; table[0xE9] = op_jp;
    // CALL family
    table[0xCD] = op_call; table[0xC4] = op_call; table[0xCC] = op_call; table[0xD4] = op_call; table[0xDC] = op_call;
    // RET/RETI family
    table[0xC9] = op_ret; table[0xD9] = op_ret; table[0xC0] = op_ret; table[0xC8] = op_ret; table[0xD0] = op_ret; table[0xD8] = op_ret;
    // RST
    table[0xC7] = op_rst; table[0xCF] = op_rst; table[0xD7] = op_rst; table[0xDF] = op_rst; table[0xE7] = op_rst; table[0xEF] = op_rst; table[0xF7] = op_rst; table[0xFF] = op_rst;
    // Misc ops
    table[0x27] = op_misc; table[0x2F] = op_misc; table[0x37] = op_misc; table[0x3F] = op_misc; table[0xF3] = op_misc; table[0xFB] = op_misc; table[0x10] = op_misc;
    // LD A,(BC/DE) and LD (BC/DE),A
    table[0x0A] = op_ld_a_rr_ind; table[0x1A] = op_ld_a_rr_ind; table[0x02] = op_ld_rr_ind_a; table[0x12] = op_ld_rr_ind_a;
    // HL +/- variants
    table[0x22] = op_ld_hl_incdec_a; table[0x2A] = op_ld_hl_incdec_a; table[0x32] = op_ld_hl_incdec_a; table[0x3A] = op_ld_hl_incdec_a;
    table[0x34] = op_incdec_hl_mem; table[0x35] = op_incdec_hl_mem;
    // CB prefix
    table[0xCB] = op_cb;
    // ADD SP,e8 (0xE8)
    table[0xE8] = |cpu: &mut CPU, _| { cpu.reg.sp = cpu.alu_add16imm(cpu.reg.sp); 4 };
    table
};

#[cfg(test)]
mod test {
    use super::CPU;
    use crate::mbc;

    const CPUINSTRS: &'static str = "test/cpu_instrs.gb";
    const GPU_CLASSIC_CHECKSUM: u32 = 3112234583;
    const GPU_COLOR_CHECKSUM: u32 = 938267576;

    #[test]
    fn cpu_instrs_classic() {
        let mut sum_classic = 0_u32;
        {
            let cart = mbc::FileBackedMBC::new(CPUINSTRS.into(), false).unwrap();
            let mut c = match CPU::new(Box::new(cart), None) {
                Err(message) => {
                    panic!("{}", message);
                }
                Ok(cpu) => cpu,
            };
            let mut ticks = 0;
            while ticks < 63802933 * 4 {
                ticks += c.do_cycle();
            }
            let fb = c.mmu.gpu.front_buffer();
            for i in 0..fb.len() {
                sum_classic = sum_classic.wrapping_add((fb[i] as u32).wrapping_mul(i as u32));
            }
        }

        assert!(
            sum_classic == GPU_CLASSIC_CHECKSUM,
            "GPU did not produce expected graphics"
        );
    }

    #[test]
    fn cpu_instrs_color() {
        let mut sum_color = 0_u32;

        {
            let cart = mbc::FileBackedMBC::new(CPUINSTRS.into(), false).unwrap();
            let mut c = match CPU::new_cgb(Box::new(cart), None) {
                Err(message) => {
                    panic!("{}", message);
                }
                Ok(cpu) => cpu,
            };
            let mut ticks = 0;
            while ticks < 63802933 * 2 {
                ticks += c.do_cycle();
            }
            let fb = c.mmu.gpu.front_buffer();
            for i in 0..fb.len() {
                sum_color = sum_color.wrapping_add((fb[i] as u32).wrapping_mul(i as u32));
            }
        }

        assert!(
            sum_color == GPU_COLOR_CHECKSUM,
            "GPU did not produce expected graphics"
        );
    }
}
