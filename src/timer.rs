/// See `Timer::reloaded_window`.
const RELOADED_WINDOW: u8 = 4;

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Timer {
    internal_counter: u16,
    tima: u8,
    tma: u8,
    tac: u8,
    reload_pending: u8,
    // T-cycles left in the M-cycle where TMA was just loaded into TIMA:
    // TIMA writes are ignored then, and TMA writes go to TIMA as well.
    reloaded_window: u8,
    prev_tima_input: bool,
    pub interrupt: u8,
}

impl Timer {
    pub fn new() -> Timer {
        Timer {
            internal_counter: 0,
            tima: 0,
            tma: 0,
            tac: 0,
            reload_pending: 0,
            reloaded_window: 0,
            prev_tima_input: false,
            interrupt: 0,
        }
    }

    pub fn rb(&self, a: u16) -> u8 {
        match a {
            0xFF04 => (self.internal_counter >> 8) as u8,
            0xFF05 => self.tima,
            0xFF06 => self.tma,
            0xFF07 => self.tac | 0xF8,
            _ => {
                debug_assert!(false, "timer rb {:04X}", a);
                0xFF
            }
        }
    }

    pub fn wb(&mut self, a: u16, v: u8) {
        match a {
            0xFF04 => {
                // Writing to DIV resets the internal counter.
                // If the bit being monitored was 1, this can cause a falling edge
                // and increment TIMA — handled in do_cycle via prev_tima_input.
                // For simplicity we also update prev_tima_input here to avoid
                // a spurious increment on the next do_cycle call.
                let tac_enabled = (self.tac & 0x04) != 0;
                let selector_bit = Self::selector_bit(self.tac);
                let old_input = tac_enabled && ((self.internal_counter >> selector_bit) & 1 == 1);
                self.internal_counter = 0;
                // After reset, bit is 0 → new_input = false.
                // If there was a falling edge (old was 1), increment TIMA now.
                if old_input {
                    let (next, overflow) = self.tima.overflowing_add(1);
                    self.tima = next;
                    if overflow {
                        self.tima = 0;
                        self.reload_pending = 4;
                    }
                }
                self.prev_tima_input = false;
            }
            0xFF05 => {
                // Pan Docs: a TIMA write during the reload window cancels the
                // reload — no TMA load, no IRQ. In the M-cycle where TMA is
                // loaded the write is ignored. Otherwise an ordinary write.
                if self.reloaded_window == 0 {
                    self.tima = v;
                }
                self.reload_pending = 0;
            }
            0xFF06 => {
                self.tma = v;
                // Written while TMA is being loaded: TIMA gets the new value.
                if self.reloaded_window > 0 {
                    self.tima = v;
                }
            }
            0xFF07 => {
                // Changing TAC can also cause a falling edge on the monitored bit.
                let tac_enabled = (self.tac & 0x04) != 0;
                let old_selector = Self::selector_bit(self.tac);
                let old_input = tac_enabled && ((self.internal_counter >> old_selector) & 1 == 1);
                self.tac = v & 0x07;
                let new_tac_enabled = (self.tac & 0x04) != 0;
                let new_selector = Self::selector_bit(self.tac);
                let new_input = new_tac_enabled && ((self.internal_counter >> new_selector) & 1 == 1);
                if old_input && !new_input {
                    let (next, overflow) = self.tima.overflowing_add(1);
                    self.tima = next;
                    if overflow {
                        self.tima = 0;
                        self.reload_pending = 4;
                    }
                }
                self.prev_tima_input = new_input;
            }
            _ => debug_assert!(false, "timer wb {:04X}", a),
        }
    }

    /// Set the divider's full 16-bit counter (post-boot state).
    pub fn set_internal_counter(&mut self, value: u16) {
        self.internal_counter = value;
        self.prev_tima_input = false;
    }

    fn selector_bit(tac: u8) -> u16 {
        match tac & 0x03 {
            0 => 9,
            1 => 3,
            2 => 5,
            3 => 7,
            _ => unreachable!(),
        }
    }

    /// Advance the timer by `ticks` t-cycles. Sets `self.interrupt` if the
    /// timer interrupt fires; the MMU will OR this into `intf` after the call.
    pub fn do_cycle(&mut self, ticks: u32) {
        if self.advance_bulk(ticks) {
            return;
        }
        self.advance_ticks(ticks);
    }

    /// T-cycles until the edge that overflows TIMA (the timer's only
    /// interrupt source). `u32::MAX` while stopped; 0 while a reload is in
    /// flight or the input latch is out of step with the counter.
    pub fn ticks_until_event(&self) -> u32 {
        if self.reload_pending != 0 {
            return 0;
        }
        let enabled = self.tac & 0x04 != 0;
        let bit = Self::selector_bit(self.tac);
        let counter = self.internal_counter as u32;
        if self.prev_tima_input != (enabled && (counter >> bit) & 1 == 1) {
            return 0;
        }
        if !enabled {
            return u32::MAX;
        }
        // Falling edges happen when the counter reaches a multiple of `period`.
        let period = 1u32 << (bit + 1);
        let to_first_edge = period - counter % period;
        to_first_edge + (0xFF - self.tima as u32) * period
    }

    /// Advance `ticks` T-cycles in O(1) when nothing tick-exact can happen in
    /// the window: no reload pending, no TIMA overflow, and the latched input
    /// agrees with the counter (a TAC/DIV write can leave them out of step,
    /// which the per-tick path turns into a glitch increment). Returns false
    /// without changing anything when the per-tick path is needed.
    fn advance_bulk(&mut self, ticks: u32) -> bool {
        if self.reload_pending != 0 {
            return false;
        }
        let enabled = self.tac & 0x04 != 0;
        let bit = Self::selector_bit(self.tac);
        let counter = self.internal_counter as u32;
        if self.prev_tima_input != (enabled && (counter >> bit) & 1 == 1) {
            return false;
        }
        let end = counter + ticks;
        self.reloaded_window = self.reloaded_window.saturating_sub(ticks.min(0xFF) as u8);
        if enabled {
            // Falling edges of the selected bit = multiples of 2^(bit+1) crossed.
            // 0x10000 is such a multiple, so u16 wraparound needs no care.
            let edges = (end >> (bit + 1)) - (counter >> (bit + 1));
            let tima = self.tima as u32 + edges;
            if tima > 0xFF {
                return false;
            }
            self.tima = tima as u8;
            self.prev_tima_input = (end >> bit) & 1 == 1;
        }
        self.internal_counter = end as u16;
        true
    }

    fn advance_ticks(&mut self, ticks: u32) {
        for _ in 0..ticks {
            // Process reload window first (reload_pending counts down in t-cycles).
            if self.reloaded_window > 0 {
                self.reloaded_window -= 1;
            }
            if self.reload_pending > 0 {
                self.reload_pending -= 1;
                if self.reload_pending == 0 {
                    self.tima = self.tma;
                    self.interrupt |= 0x04;
                    self.reloaded_window = RELOADED_WINDOW;
                }
            }

            self.internal_counter = self.internal_counter.wrapping_add(1);

            let tac_enabled = (self.tac & 0x04) != 0;
            let selector_bit = Self::selector_bit(self.tac);
            let new_input = tac_enabled && ((self.internal_counter >> selector_bit) & 1 == 1);

            // Falling edge on the monitored bit → increment TIMA.
            if self.prev_tima_input && !new_input {
                let (next, overflow) = self.tima.overflowing_add(1);
                self.tima = next;
                if overflow {
                    self.tima = 0;
                    self.reload_pending = 4;
                }
            }
            self.prev_tima_input = new_input;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writing any value to FF04 (DIV) must reset the internal counter,
    /// making DIV read back as 0.
    #[test]
    fn timer_div_resets_on_ff04_write() {
        let mut t = Timer::new();
        // Advance counter so DIV is non-zero.
        t.do_cycle(512); // 512 t-cycles → internal_counter = 512, DIV = 512>>8 = 1
        assert_ne!(t.rb(0xFF04), 0, "DIV should be non-zero after 512 t-cycles");
        t.wb(0xFF04, 0x00);
        assert_eq!(t.rb(0xFF04), 0, "DIV must be 0 after write to FF04");
    }

    /// At TAC = 0x04 (enabled, mode 0 = 4096 Hz), the monitored bit is bit 9 of
    /// internal_counter. Bit 9 has period 1024 t-cycles (512 high, 512 low).
    /// Each falling edge increments TIMA. After 1024 t-cycles TIMA should be 1,
    /// after 2048 t-cycles TIMA should be 2, etc.
    ///
    /// TAC encoding: bit 2 = enable, bits 1:0 = clock_select.
    ///   0x04 = enable=1, mode=0 (4096 Hz, monitors bit 9).
    ///   0x05 = enable=1, mode=1 (262144 Hz, monitors bit 3).
    #[test]
    fn tima_increments_at_correct_rate_for_tac_0x04() {
        let mut t = Timer::new();
        t.wb(0xFF07, 0x04); // enable, mode 0 = 4096 Hz (bit 9 of internal_counter)
        // Run 1024 t-cycles: internal_counter goes 1..1024.
        // Bit 9 rises at counter=512 (0→1) and falls at counter=1024→0 (wraps).
        // That is one falling edge → TIMA increments once.
        t.do_cycle(1024);
        assert_eq!(t.tima, 1, "TIMA should be 1 after one 1024-cycle period");
        t.do_cycle(1024);
        assert_eq!(t.tima, 2, "TIMA should be 2 after two 1024-cycle periods");
        t.do_cycle(1024 * 253);
        assert_eq!(t.tima, 255, "TIMA should be 255 after 255 periods");
    }

    /// When TIMA overflows (0xFF→0x00), reload_pending is set. The interrupt
    /// should fire only after the reload window has elapsed (4 t-cycles), and
    /// TIMA should be set to TMA at that point.
    ///
    /// Uses TAC=0x04 (mode 0, bit 9). With internal_counter starting at 0:
    ///   - After 512 t-cycles: bit 9 goes 0→1 (rising edge, no TIMA change).
    ///   - After 512 more (total 1024): internal_counter wraps to 0, bit 9 falls
    ///     1→0 → TIMA overflows → reload_pending = 4.
    ///   - TIMA reads 0 during the reload window; interrupt is NOT yet set.
    ///   - After 4 more t-cycles: reload fires, TIMA = TMA, interrupt bit set.
    #[test]
    fn tima_overflow_triggers_interrupt_after_reload_window() {
        let mut t = Timer::new();
        t.wb(0xFF06, 0x42); // TMA = 0x42
        t.wb(0xFF07, 0x04); // enable, mode 0 = 4096 Hz (bit 9)
        // Pre-load TIMA to 0xFF via direct register write (reload_pending is 0).
        t.wb(0xFF05, 0xFF);
        assert_eq!(t.tima, 0xFF);

        // Run 512 cycles: bit 9 goes 0→1 (rising edge only, no TIMA increment).
        t.do_cycle(512);
        assert_eq!(t.interrupt, 0, "no interrupt after rising edge");
        assert_eq!(t.tima, 0xFF, "TIMA unchanged after rising edge");

        // Run 511 more cycles: bit 9 still high, no falling edge yet.
        t.do_cycle(511);
        assert_eq!(t.tima, 0xFF, "TIMA still 0xFF before overflow cycle");
        assert_eq!(t.interrupt, 0, "no interrupt before overflow");

        // Run the 1 cycle that wraps internal_counter to 0 (the falling edge):
        // TIMA 0xFF + 1 overflows to 0, reload_pending = 4.
        t.do_cycle(1);
        assert_eq!(t.tima, 0, "TIMA should be 0 immediately after overflow");
        assert_eq!(t.interrupt, 0, "interrupt NOT yet fired during reload window");
        assert!(t.reload_pending > 0, "reload_pending should be set");

        // Advance 3 more t-cycles (still within the 4-cycle window).
        t.do_cycle(3);
        assert_eq!(t.interrupt, 0, "interrupt still not fired after 3 of 4 reload cycles");

        // The 4th cycle exhausts the reload window.
        t.do_cycle(1);
        assert_eq!(t.interrupt & 0x04, 0x04, "timer interrupt should be set after reload window");
        assert_eq!(t.tima, 0x42, "TIMA should be loaded from TMA after reload window");
    }

    /// Writing to TIMA during the 4-t-cycle reload window cancels the reload:
    /// no TMA load, no timer interrupt (Pan Docs §Timer overflow behaviour).
    #[test]
    fn tima_write_during_reload_window_cancels_reload() {
        let mut t = Timer::new();
        t.wb(0xFF06, 0xAB); // TMA = 0xAB
        t.wb(0xFF07, 0x04); // enable, mode 0 = 4096 Hz (bit 9)
        t.wb(0xFF05, 0xFF); // TIMA = 0xFF
        t.do_cycle(512); // bit 9: 0→1
        t.do_cycle(512); // bit 9: 1→0 → overflow → reload_pending = 4

        assert!(t.reload_pending > 0, "should be in reload window");
        assert_eq!(t.tima, 0, "TIMA is 0 during reload window");

        // Cancel the reload by writing TIMA.
        t.wb(0xFF05, 0x33);
        assert_eq!(t.tima, 0x33, "TIMA write during reload must take effect");
        assert_eq!(t.reload_pending, 0, "reload must be cancelled by the write");

        // Exhaust the original 4-cycle window: reload must NOT fire.
        t.do_cycle(4);
        assert_eq!(t.tima, 0x33, "TIMA must not be overwritten by TMA after cancel");
        assert_eq!(t.interrupt & 0x04, 0, "no timer IRQ after cancelled reload");
    }
}
