//! Windows backend: Winsock RFCOMM socket and a ViGEmBus virtual Xbox 360 controller.

use super::{is_g4_name, Addr, Link, Pad, Recv};
use crate::report::PadState;
use std::mem::{size_of, zeroed};
use std::sync::Once;
use windows_sys::Win32::Devices::Bluetooth::{
    BluetoothFindDeviceClose, BluetoothFindFirstDevice, BluetoothFindNextDevice, BLUETOOTH_DEVICE_INFO,
    BLUETOOTH_DEVICE_SEARCH_PARAMS,
};
use windows_sys::Win32::Networking::WinSock::{
    closesocket, connect, recv, setsockopt, socket, WSAGetLastError, WSAStartup, INVALID_SOCKET, SOCKADDR, SOCKET,
    SOCKET_ERROR, SOCK_STREAM, SOL_SOCKET, SO_RCVTIMEO, WSADATA,
};

const AF_BTH: u16 = 32;
const BTHPROTO_RFCOMM: i32 = 3;
/// RFCOMM channel of the pad's "WirelessCom" serial service.
const CHANNEL: u32 = 1;
const WSAETIMEDOUT: i32 = 10060;

/// `SOCKADDR_BTH`, declared here so the packing is explicit.
#[repr(C, packed(1))]
struct SockAddrBth {
    address_family: u16,
    bt_addr: u64,
    service_class_id: [u8; 16],
    port: u32,
}

fn wsa_init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        let mut data: WSADATA = zeroed();
        WSAStartup(0x0202, &mut data);
    });
}

pub fn describe_error(code: i32) -> &'static str {
    match code {
        10060 => "pad not reachable - is it on and blinking? (hold A + HOME)",
        10051 => "bond mismatch - remove \"Gamesir-G4\" in Windows Bluetooth settings, hold A + HOME, pair again",
        10053 => "connection aborted",
        10054 => "pad closed the connection",
        10064 => "pad does not answer (switched off?)",
        _ => "",
    }
}

/// Address of a paired device whose name looks like a G4.
pub fn find_paired() -> Option<Addr> {
    unsafe {
        let mut params: BLUETOOTH_DEVICE_SEARCH_PARAMS = zeroed();
        params.dwSize = size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32;
        params.fReturnAuthenticated = 1;
        params.fReturnRemembered = 1;
        params.fReturnConnected = 1;
        let mut info: BLUETOOTH_DEVICE_INFO = zeroed();
        info.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
        let handle = BluetoothFindFirstDevice(&params, &mut info);
        if handle.is_null() {
            return None;
        }
        let mut found = None;
        loop {
            let len = info.szName.iter().position(|c| *c == 0).unwrap_or(info.szName.len());
            if is_g4_name(&String::from_utf16_lossy(&info.szName[..len])) {
                found = Some(info.Address.Anonymous.ullLong);
                break;
            }
            if BluetoothFindNextDevice(handle, &mut info) == 0 {
                break;
            }
        }
        BluetoothFindDeviceClose(handle);
        found
    }
}

struct RfcommLink(SOCKET);

impl Link for RfcommLink {
    fn recv(&mut self, buf: &mut [u8]) -> Recv {
        let n = unsafe { recv(self.0, buf.as_mut_ptr(), buf.len() as i32, 0) };
        if n == 0 {
            Recv::Closed(None)
        } else if n < 0 {
            let e = unsafe { WSAGetLastError() };
            if e == WSAETIMEDOUT {
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
            closesocket(self.0);
        }
    }
}

/// Open the serial channel. `Err` carries the WSA error code.
pub fn connect_pad(addr: Addr) -> Result<Box<dyn Link>, i32> {
    wsa_init();
    unsafe {
        let s = socket(i32::from(AF_BTH), SOCK_STREAM, BTHPROTO_RFCOMM);
        if s == INVALID_SOCKET {
            return Err(WSAGetLastError());
        }
        let sa = SockAddrBth { address_family: AF_BTH, bt_addr: addr, service_class_id: [0; 16], port: CHANNEL };
        if connect(s, &sa as *const SockAddrBth as *const SOCKADDR, size_of::<SockAddrBth>() as i32) == SOCKET_ERROR {
            let e = WSAGetLastError();
            closesocket(s);
            return Err(e);
        }
        let timeout_ms: u32 = 1000;
        setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &timeout_ms as *const u32 as *const u8, 4);
        Ok(Box::new(RfcommLink(s)))
    }
}

struct VigemPad {
    target: vigem_client::Xbox360Wired<vigem_client::Client>,
}

impl Pad for VigemPad {
    fn update(&mut self, s: &PadState) {
        let gp = vigem_client::XGamepad {
            buttons: vigem_client::XButtons { raw: s.buttons },
            left_trigger: s.lt,
            right_trigger: s.rt,
            thumb_lx: s.lx,
            thumb_ly: s.ly,
            thumb_rx: s.rx,
            thumb_ry: s.ry,
        };
        let _ = self.target.update(&gp);
    }
}

/// Plug in a virtual Xbox 360 controller.
pub fn open_pad() -> Result<Box<dyn Pad>, String> {
    let client = vigem_client::Client::connect().map_err(|e| {
        format!("cannot talk to ViGEmBus ({e:?}). Install the driver: https://github.com/nefarius/ViGEmBus/releases")
    })?;
    let mut target = vigem_client::Xbox360Wired::new(client, vigem_client::TargetId::XBOX360_WIRED);
    target.plugin().map_err(|e| format!("cannot plug in the virtual controller: {e:?}"))?;
    target.wait_ready().map_err(|e| format!("virtual controller not ready: {e:?}"))?;
    Ok(Box::new(VigemPad { target }))
}
