#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Serial {
    data: u8,
    control: u8,
    pub interrupt: u8,
    /// Bits still to shift in the current transfer (0 = idle).
    bits_left: u8,
    /// Bytes sent over the link port. Only recorded in test builds, where the
    /// test-ROM harness reads Blargg's serial output from it.
    #[rkyv(with = rkyv::with::Skip)]
    pub output: Vec<u8>,
}

impl Serial {
    pub fn wb(&mut self, a: u16, v: u8) {
        match a {
            0xFF01 => self.data = v,
            0xFF02 => {
                self.control = v;
                // Internal clock: shift 8 bits on the serial clock (see
                // `clock`). External clock with no peer: never completes.
                self.bits_left = if v & 0x81 == 0x81 { 8 } else { 0 };
                #[cfg(test)]
                if self.bits_left == 8 {
                    self.output.push(self.data);
                }
            }
            _ => panic!("Serial does not handle address {:4X} (write)", a),
        };
    }

    /// Whether an internally clocked transfer is in progress.
    pub fn transferring(&self) -> bool {
        self.bits_left > 0
    }

    /// `edges` falling edges of the 8192 Hz serial clock. With no link
    /// partner every bit shifts in as 1; after 8 the transfer completes.
    pub fn clock(&mut self, edges: u32) {
        for _ in 0..edges.min(self.bits_left as u32) {
            self.data = (self.data << 1) | 1;
            self.bits_left -= 1;
            if self.bits_left == 0 {
                self.control &= 0x7F; // SC bit 7 clears on completion
                self.interrupt = 0x08;
            }
        }
    }

    pub fn rb(&self, a: u16) -> u8 {
        match a {
            0xFF01 => self.data,
            0xFF02 => self.control | 0b01111110,
            _ => panic!("Serial does not handle address {:4X} (read)", a),
        }
    }

    pub fn new() -> Serial {
        Serial {
            data: 0,
            control: 0,
            interrupt: 0,
            bits_left: 0,
            output: Vec::new(),
        }
    }
}
