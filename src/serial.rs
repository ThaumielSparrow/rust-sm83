#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Serial {
    data: u8,
    control: u8,
    pub interrupt: u8,
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
                if v & 0x81 == 0x81 {
                    #[cfg(test)]
                    self.output.push(self.data);
                    // No link/printer; emulate instant transfer complete.
                    self.interrupt = 0x8;
                    // No peer: bits shift in as 1s, leaving SB at 0xFF.
                    self.data = 0xFF;
                    // SC bit 7 (transfer-start) auto-clears on completion.
                    self.control &= 0x7F;
                }
            }
            _ => panic!("Serial does not handle address {:4X} (write)", a),
        };
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
            output: Vec::new(),
        }
    }
}
