// Interrupt helper utilities
// IE at 0xFFFF is in Memory.interrupt_enable, IF is at io_registers[0x0F]

#[derive(Copy, Clone)]
pub enum Interrupt {
    VBlank = 0,
    LCDStat = 1,
    Timer = 2,
    Serial = 3,
    Joypad = 4,
}

pub fn highest_pending_interrupt(mem: &crate::cpu::mmu::Memory) -> Option<Interrupt> {
    let ie = mem.interrupt_enable;
    let iflag = mem.io_registers[0x0F];
    let pending = ie & iflag;
    if pending == 0 { return None; }

    for i in 0..5u8 {
        if (pending & (1 << i)) != 0 {
            return match i {
                0 => Some(Interrupt::VBlank),
                1 => Some(Interrupt::LCDStat),
                2 => Some(Interrupt::Timer),
                3 => Some(Interrupt::Serial),
                4 => Some(Interrupt::Joypad),
                _ => None,
            };
        }
    }
    None
}
