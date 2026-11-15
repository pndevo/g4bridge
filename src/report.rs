//! Platform independent part: HID report parsing and mapping to an XInput style pad state.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Header byte of a Bluetooth HID "DATA | Input" transaction.
const HEADER: u8 = 0xA1;
/// Report ID of the gamepad report.
pub const ID_PAD: u8 = 0xC4;

/// XInput button bit masks (same values as `XINPUT_GAMEPAD_*`).
pub mod xb {
    pub const UP: u16 = 0x0001;
    pub const DOWN: u16 = 0x0002;
    pub const LEFT: u16 = 0x0004;
    pub const RIGHT: u16 = 0x0008;
    pub const START: u16 = 0x0010;
    pub const BACK: u16 = 0x0020;
    pub const L3: u16 = 0x0040;
    pub const R3: u16 = 0x0080;
    pub const LB: u16 = 0x0100;
    pub const RB: u16 = 0x0200;
    pub const GUIDE: u16 = 0x0400;
    pub const A: u16 = 0x1000;
    pub const B: u16 = 0x2000;
    pub const X: u16 = 0x4000;
    pub const Y: u16 = 0x8000;
}

/// Button name, XInput mask, default bit in the 16 bit button field (Android gamepad layout).
pub const BUTTONS: [(&str, u16, u8); 11] = [
    ("A", xb::A, 0),
    ("B", xb::B, 1),
    ("X", xb::X, 3),
    ("Y", xb::Y, 4),
    ("LB", xb::LB, 6),
    ("RB", xb::RB, 7),
    ("BACK", xb::BACK, 10),
    ("START", xb::START, 11),
    ("GUIDE", xb::GUIDE, 12),
    ("L3", xb::L3, 13),
    ("R3", xb::R3, 14),
];

/// Expected packet length including the 0xA1 header byte, or `None` for an unknown report ID.
fn packet_len(id: u8) -> Option<usize> {
    match id {
        ID_PAD => Some(12),
        0x12 => Some(5), // consumer control
        0x06 => Some(6), // mouse
        _ => None,
    }
}

/// One report from the serial stream (header stripped).
pub struct Packet {
    pub id: u8,
    pub payload: Vec<u8>,
}

/// Reassembles reports from an RFCOMM byte stream; resynchronises on the header byte.
#[derive(Default)]
pub struct Parser {
    buf: Vec<u8>,
}

impl Parser {
    pub fn push(&mut self, data: &[u8]) -> Vec<Packet> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            if self.buf.is_empty() {
                break;
            }
            if self.buf[0] != HEADER {
                self.buf.remove(0);
                continue;
            }
            if self.buf.len() < 2 {
                break;
            }
            let Some(len) = packet_len(self.buf[1]) else {
                self.buf.remove(0);
                continue;
            };
            if self.buf.len() < len {
                break;
            }
            out.push(Packet { id: self.buf[1], payload: self.buf[2..len].to_vec() });
            self.buf.drain(..len);
        }
        out
    }
}

/// Decoded gamepad report 0xC4.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct PadReport {
    pub lx: u8,
    pub ly: u8,
    pub rx: u8,
    pub ry: u8,
    pub lt: u8,
    pub rt: u8,
    pub buttons: u16,
    pub hat: u8,
}

impl PadReport {
    pub fn from_packet(p: &Packet) -> Option<Self> {
        if p.id != ID_PAD || p.payload.len() < 9 {
            return None;
        }
        let d = &p.payload;
        Some(Self {
            lx: d[0],
            ly: d[1],
            rx: d[2],
            ry: d[3],
            lt: d[4],
            rt: d[5],
            buttons: u16::from(d[6]) | (u16::from(d[7]) << 8),
            hat: d[8] & 0x0F,
        })
    }
}

/// Output state in XInput terms.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct PadState {
    pub buttons: u16,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}

/// 8 bit HID axis (0..255, 128 = centre) to XInput i16. Inversion happens before clamping.
pub fn axis(v: u8, invert: bool) -> i16 {
    let mut val = (i32::from(v) - 128) * 258;
    if invert {
        val = -val;
    }
    val.clamp(-32768, 32767) as i16
}

/// Hat switch value (1 = north, clockwise; 0 / 15 = released) to XInput D-pad bits.
pub fn hat_bits(hat: u8) -> u16 {
    match hat {
        1 => xb::UP,
        2 => xb::UP | xb::RIGHT,
        3 => xb::RIGHT,
        4 => xb::DOWN | xb::RIGHT,
        5 => xb::DOWN,
        6 => xb::DOWN | xb::LEFT,
        7 => xb::LEFT,
        8 => xb::UP | xb::LEFT,
        _ => 0,
    }
}

/// Button name -> bit index in the pad's 16 bit button field. Stored as `bt_map.json`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Mapping(pub BTreeMap<String, u8>);

impl Default for Mapping {
    fn default() -> Self {
        Mapping(BUTTONS.iter().map(|(n, _, bit)| ((*n).to_string(), *bit)).collect())
    }
}

impl Mapping {
    /// Defaults overlaid with the entries found in `json`.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let mut m = Mapping::default();
        let over: BTreeMap<String, u8> = serde_json::from_str(json)?;
        m.0.extend(over);
        Ok(m)
    }

    pub fn apply(&self, r: &PadReport) -> PadState {
        let mut buttons = hat_bits(r.hat);
        for (name, mask, _) in BUTTONS {
            if let Some(&bit) = self.0.get(name) {
                if bit < 16 && r.buttons & (1 << bit) != 0 {
                    buttons |= mask;
                }
            }
        }
        PadState {
            buttons,
            lt: r.lt,
            rt: r.rt,
            lx: axis(r.lx, false),
            ly: axis(r.ly, true),
            rx: axis(r.rx, false),
            ry: axis(r.ry, true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad_packet(extra: [u8; 10]) -> Vec<u8> {
        let mut v = vec![HEADER, ID_PAD];
        v.extend_from_slice(&extra);
        v
    }

    #[test]
    fn axis_centre_and_limits() {
        assert_eq!(axis(128, false), 0);
        assert_eq!(axis(255, false), 32766);
        assert_eq!(axis(0, false), -32768);
        assert_eq!(axis(0, true), 32767);
        assert_eq!(axis(255, true), -32766);
    }

    #[test]
    fn hat_directions() {
        assert_eq!(hat_bits(0), 0);
        assert_eq!(hat_bits(15), 0);
        assert_eq!(hat_bits(1), xb::UP);
        assert_eq!(hat_bits(4), xb::DOWN | xb::RIGHT);
        assert_eq!(hat_bits(8), xb::UP | xb::LEFT);
    }

    #[test]
    fn parses_single_packet() {
        let mut p = Parser::default();
        let out = p.push(&pad_packet([128, 128, 128, 128, 255, 0, 0x01, 0x01, 3, 0]));
        assert_eq!(out.len(), 1);
        let r = PadReport::from_packet(&out[0]).unwrap();
        assert_eq!(r.lt, 255);
        assert_eq!(r.buttons, 0x0101);
        assert_eq!(r.hat, 3);
    }

    #[test]
    fn handles_split_and_garbage() {
        let mut p = Parser::default();
        let full = pad_packet([128; 10]);
        assert!(p.push(&[0x00, 0x55]).is_empty());
        assert!(p.push(&full[..5]).is_empty());
        let out = p.push(&full[5..]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn skips_other_reports_and_unknown_ids() {
        let mut p = Parser::default();
        let mut data = vec![HEADER, 0x12, 1, 2, 3, HEADER, 0x99];
        data.extend(pad_packet([128; 10]));
        let out = p.push(&data);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, 0x12);
        assert_eq!(out[1].id, ID_PAD);
        assert!(PadReport::from_packet(&out[0]).is_none());
    }

    #[test]
    fn default_mapping_and_override() {
        let r = PadReport { lx: 128, ly: 128, rx: 128, ry: 128, lt: 0, rt: 0, buttons: 1, hat: 0 };
        assert_eq!(Mapping::default().apply(&r).buttons, xb::A);
        let m = Mapping::from_json(r#"{"A": 5, "B": 0}"#).unwrap();
        assert_eq!(m.apply(&r).buttons, xb::B);
    }

    #[test]
    fn out_of_range_bit_is_ignored() {
        let r = PadReport { lx: 128, ly: 128, rx: 128, ry: 128, lt: 0, rt: 0, buttons: 0xFFFF, hat: 0 };
        let m = Mapping::from_json(r#"{"A": 99}"#).unwrap();
        assert_eq!(m.apply(&r).buttons & xb::A, 0);
    }
}
