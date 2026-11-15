//! Operating system specific parts: Bluetooth RFCOMM link and virtual gamepad output.
//!
//! Each backend exports the same four functions: `find_paired`, `connect_pad`, `describe_error`, `open_pad`.

use crate::report::PadState;

/// 48 bit Bluetooth address, most significant byte first (`AA:BB:CC:DD:EE:FF` = `0xAABBCCDDEEFF`).
pub type Addr = u64;

pub enum Recv {
    /// `n` bytes were written to the buffer.
    Data(usize),
    /// Receive timeout: the pad is idle (it only sends on change), the link is still up.
    Idle,
    /// The link is gone; carries the OS error code if there was one.
    Closed(Option<i32>),
}

/// An open serial (RFCOMM) link to the pad.
pub trait Link {
    fn recv(&mut self, buf: &mut [u8]) -> Recv;
}

/// A virtual gamepad the games see.
pub trait Pad {
    fn update(&mut self, state: &PadState);
    fn reset(&mut self) {
        self.update(&PadState::default());
    }
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(not(any(windows, target_os = "linux")))]
compile_error!("g4bridge supports Windows and Linux only");

/// True if a Bluetooth device name looks like a GameSir G4 ("Gamesir-G4", "GameSir G4", ...).
pub fn is_g4_name(name: &str) -> bool {
    let n: String = name.to_lowercase().chars().filter(|c| *c != '-' && *c != ' ').collect();
    n.contains("gamesirg4")
}

#[cfg(test)]
mod tests {
    use super::is_g4_name;

    #[test]
    fn name_matching() {
        assert!(is_g4_name("Gamesir-G4"));
        assert!(is_g4_name("GameSir G4"));
        assert!(!is_g4_name("DualSense Wireless Controller"));
    }
}
