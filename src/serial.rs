use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Serial {
    data: u8,
    control: u8,
    // External callback support removed.
    pub interrupt: u8,
}

impl Serial {
    pub fn wb(&mut self, a: u16, v: u8) {
        match a {
            0xFF01 => self.data = v,
            0xFF02 => {
                self.control = v;
                if v & 0x81 == 0x81 {
                    // No link/printer; emulate instant transfer complete.
                    self.interrupt = 0x8;
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
        }
    }
}
