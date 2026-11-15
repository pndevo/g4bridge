//! Linux backend (experimental, untested on real hardware):
//! BlueZ RFCOMM socket and a virtual gamepad through `/dev/uinput`.

use super::{is_g4_name, Addr, Link, Pad, Recv};
use crate::report::{xb, PadState};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::mem::{size_of, zeroed};
use std::os::unix::io::AsRawFd;
use std::process::Command;

const AF_BLUETOOTH: i32 = 31;
const BTPROTO_RFCOMM: i32 = 3;
/// RFCOMM channel of the pad's "WirelessCom" serial service.
const CHANNEL: u8 = 1;

pub fn describe_error(code: i32) -> &'static str {
    match code {
        libc::ETIMEDOUT | libc::EHOSTDOWN => "pad not reachable - is it on and blinking? (hold A + HOME)",
        libc::ECONNREFUSED => "pad refused the connection - pair it again (bluetoothctl remove, then pair)",
        libc::EHOSTUNREACH => "pad not in range or Bluetooth is off",
        libc::ECONNRESET => "pad closed the connection",
        _ => "",
    }
}

/// Address of a paired device whose name looks like a G4 (parsed from `bluetoothctl`).
pub fn find_paired() -> Option<Addr> {
    for args in [&["devices", "Paired"][..], &["paired-devices"][..]] {
        let Ok(out) = Command::new("bluetoothctl").args(args).output() else { continue };
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            // "Device AA:BB:CC:DD:EE:FF Gamesir-G4"
            let mut it = line.trim().splitn(3, ' ');
            if it.next() != Some("Device") {
                continue;
            }
            let (Some(mac), Some(name)) = (it.next(), it.next()) else { continue };
            if is_g4_name(name) {
                return u64::from_str_radix(&mac.replace(':', ""), 16).ok();
            }
        }
    }
    None
}

#[repr(C, packed)]
struct SockAddrRc {
    family: u16,
    bdaddr: [u8; 6], // little endian: least significant byte first
    channel: u8,
}

struct RfcommLink(i32);

impl Link for RfcommLink {
    fn recv(&mut self, buf: &mut [u8]) -> Recv {
        let n = unsafe { libc::recv(self.0, buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n == 0 {
            Recv::Closed(None)
        } else if n < 0 {
            let e = io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if e == libc::EAGAIN || e == libc::EWOULDBLOCK || e == libc::EINTR {
                Recv::Idle
            } else {
                Recv::Closed(Some(e))
            }
        } else {
            Recv::Data(n as usize)
        }
    }
}

impl Drop for RfcommLink {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}

/// Open the serial channel. `Err` carries the errno.
pub fn connect_pad(addr: Addr) -> Result<Box<dyn Link>, i32> {
    unsafe {
        let s = libc::socket(AF_BLUETOOTH, libc::SOCK_STREAM, BTPROTO_RFCOMM);
        if s < 0 {
            return Err(io::Error::last_os_error().raw_os_error().unwrap_or(0));
        }
        let mut bdaddr = [0u8; 6];
        for (i, b) in bdaddr.iter_mut().enumerate() {
            *b = (addr >> (8 * i)) as u8;
        }
        let sa = SockAddrRc { family: AF_BLUETOOTH as u16, bdaddr, channel: CHANNEL };
        if libc::connect(s, &sa as *const SockAddrRc as *const libc::sockaddr, size_of::<SockAddrRc>() as u32) != 0 {
            let e = io::Error::last_os_error().raw_os_error().unwrap_or(0);
            libc::close(s);
            return Err(e);
        }
        let tv = libc::timeval { tv_sec: 1, tv_usec: 0 };
        libc::setsockopt(
            s,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &tv as *const libc::timeval as *const libc::c_void,
            size_of::<libc::timeval>() as u32,
        );
        Ok(Box::new(RfcommLink(s)))
    }
}

// ---- uinput -------------------------------------------------------------------------------------
const UI_DEV_CREATE: u64 = 0x5501;
const UI_DEV_DESTROY: u64 = 0x5502;
const UI_DEV_SETUP: u64 = 0x405c_5503;
const UI_ABS_SETUP: u64 = 0x401c_5504;
const UI_SET_EVBIT: u64 = 0x4004_5564;
const UI_SET_KEYBIT: u64 = 0x4004_5565;
const UI_SET_ABSBIT: u64 = 0x4004_5567;

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_Z: u16 = 0x02;
const ABS_RX: u16 = 0x03;
const ABS_RY: u16 = 0x04;
const ABS_RZ: u16 = 0x05;
const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;

/// XInput mask -> evdev key code (same layout the kernel's xpad driver reports).
const KEYS: [(u16, u16); 11] = [
    (xb::A, 0x130),     // BTN_SOUTH
    (xb::B, 0x131),     // BTN_EAST
    (xb::X, 0x133),     // BTN_NORTH
    (xb::Y, 0x134),     // BTN_WEST
    (xb::LB, 0x136),    // BTN_TL
    (xb::RB, 0x137),    // BTN_TR
    (xb::BACK, 0x13a),  // BTN_SELECT
    (xb::START, 0x13b), // BTN_START
    (xb::GUIDE, 0x13c), // BTN_MODE
    (xb::L3, 0x13d),    // BTN_THUMBL
    (xb::R3, 0x13e),    // BTN_THUMBR
];

struct UinputPad {
    file: File,
}

fn ioctl(fd: i32, req: u64, arg: libc::c_ulong) -> io::Result<()> {
    if unsafe { libc::ioctl(fd, req as _, arg) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn abs_setup(fd: i32, code: u16, min: i32, max: i32, fuzz: i32, flat: i32) -> io::Result<()> {
    let mut a: libc::uinput_abs_setup = unsafe { zeroed() };
    a.code = code;
    a.absinfo.minimum = min;
    a.absinfo.maximum = max;
    a.absinfo.fuzz = fuzz;
    a.absinfo.flat = flat;
    if unsafe { libc::ioctl(fd, UI_ABS_SETUP as _, &a) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

impl UinputPad {
    fn new() -> io::Result<Self> {
        let file = OpenOptions::new().write(true).open("/dev/uinput")?;
        let fd = file.as_raw_fd();
        ioctl(fd, UI_SET_EVBIT, EV_KEY as _)?;
        for (_, key) in KEYS {
            ioctl(fd, UI_SET_KEYBIT, key as _)?;
        }
        ioctl(fd, UI_SET_EVBIT, EV_ABS as _)?;
        for (code, min, max) in [
            (ABS_X, -32768, 32767),
            (ABS_Y, -32768, 32767),
            (ABS_RX, -32768, 32767),
            (ABS_RY, -32768, 32767),
            (ABS_Z, 0, 255),
            (ABS_RZ, 0, 255),
            (ABS_HAT0X, -1, 1),
            (ABS_HAT0Y, -1, 1),
        ] {
            ioctl(fd, UI_SET_ABSBIT, code as _)?;
            let (fuzz, flat) = if max > 255 { (16, 128) } else { (0, 0) };
            abs_setup(fd, code, min, max, fuzz, flat)?;
        }
        // Look like a wired Xbox 360 pad so SDL / Steam pick the usual mapping.
        let mut setup: libc::uinput_setup = unsafe { zeroed() };
        setup.id.bustype = 0x03; // BUS_USB
        setup.id.vendor = 0x045e;
        setup.id.product = 0x028e;
        setup.id.version = 0x0110;
        for (i, b) in b"Microsoft X-Box 360 pad".iter().enumerate() {
            setup.name[i] = *b as libc::c_char;
        }
        if unsafe { libc::ioctl(fd, UI_DEV_SETUP as _, &setup) } < 0 {
            return Err(io::Error::last_os_error());
        }
        ioctl(fd, UI_DEV_CREATE, 0)?;
        Ok(Self { file })
    }

    fn emit(&mut self, buf: &mut Vec<u8>, kind: u16, code: u16, value: i32) {
        let mut ev: libc::input_event = unsafe { zeroed() };
        ev.type_ = kind;
        ev.code = code;
        ev.value = value;
        let bytes = unsafe { std::slice::from_raw_parts(&ev as *const _ as *const u8, size_of::<libc::input_event>()) };
        buf.extend_from_slice(bytes);
    }
}

impl Pad for UinputPad {
    fn update(&mut self, s: &PadState) {
        let mut buf = Vec::with_capacity(32 * size_of::<libc::input_event>());
        for (mask, key) in KEYS {
            self.emit(&mut buf, EV_KEY, key, i32::from(s.buttons & mask != 0));
        }
        let hat = |neg: u16, pos: u16| i32::from(s.buttons & pos != 0) - i32::from(s.buttons & neg != 0);
        for (code, value) in [
            (ABS_X, i32::from(s.lx)),
            (ABS_Y, -i32::from(s.ly)), // evdev: down is positive
            (ABS_RX, i32::from(s.rx)),
            (ABS_RY, -i32::from(s.ry)),
            (ABS_Z, i32::from(s.lt)),
            (ABS_RZ, i32::from(s.rt)),
            (ABS_HAT0X, hat(xb::LEFT, xb::RIGHT)),
            (ABS_HAT0Y, hat(xb::UP, xb::DOWN)),
        ] {
            self.emit(&mut buf, EV_ABS, code, value);
        }
        self.emit(&mut buf, EV_SYN, 0, 0);
        let _ = self.file.write_all(&buf);
    }
}

impl Drop for UinputPad {
    fn drop(&mut self) {
        let _ = ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY, 0);
    }
}

/// Create the virtual gamepad.
pub fn open_pad() -> Result<Box<dyn Pad>, String> {
    UinputPad::new().map(|p| Box::new(p) as Box<dyn Pad>).map_err(|e| {
        format!(
            "cannot create the virtual gamepad ({e}). Is /dev/uinput writable? Try: sudo modprobe uinput; \
                 sudo setfacl -m u:$USER:rw /dev/uinput"
        )
    })
}
