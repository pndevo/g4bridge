"""G4Bridge - use a GameSir G4 gamepad over Bluetooth on Windows.

Windows pairs the G4 but never installs its HID (gamepad) service, because
the pad's Bluetooth HID record violates the spec (see FINDINGS.md). The same
firmware, however, streams its gamepad input reports over a plain Bluetooth
serial channel (RFCOMM channel 1, service "WirelessCom"). Any user-mode
program may open that channel. G4Bridge does exactly that and feeds the
reports into a virtual Xbox 360 controller provided by ViGEmBus.

Usage:
    python g4bridge.py                 run; reconnects forever, Ctrl+C stops
    python g4bridge.py calibrate       learn button bits, writes bt_map.json
    python g4bridge.py --addr AA:BB:CC:DD:EE:FF
                                       skip auto-detection, use this address

Requirements: Windows 10/11, Python 3.8+, ViGEmBus driver, `pip install vgamepad`.
"""
import argparse
import ctypes
import json
import os
import re
import struct
import sys
import time
from ctypes import wintypes as w

__version__ = '0.1.0'

CHANNEL = 1                                   # RFCOMM channel of "WirelessCom"
NAME_PATTERN = re.compile(r'gamesir[- ]?g4', re.I)
HERE = os.path.dirname(os.path.abspath(__file__))
MAP_FILE = os.path.join(HERE, 'bt_map.json')

# Button bit index inside the 16-bit button field of report 0xC4.
# This is the standard Android gamepad layout; verified on a real G4.
DEFAULT_MAP = {'A': 0, 'B': 1, 'X': 3, 'Y': 4, 'LB': 6, 'RB': 7,
               'BACK': 10, 'START': 11, 'GUIDE': 12, 'L3': 13, 'R3': 14}

# Report lengths incl. the 0xA1 (DATA | Input) header byte.
REPORT_LEN = {0xC4: 12, 0x12: 5, 0x06: 6}     # gamepad, consumer, mouse

# ---------------------------------------------------------------- Win32 glue
ws = ctypes.WinDLL('ws2_32', use_last_error=True)
bt = ctypes.WinDLL('bthprops.cpl', use_last_error=True)
AF_BTH, SOCK_STREAM, BTHPROTO_RFCOMM = 32, 1, 3
SOL_SOCKET, SO_RCVTIMEO = 0xFFFF, 0x1006


class WSADATA(ctypes.Structure):
    _fields_ = [('wVersion', w.WORD), ('wHighVersion', w.WORD), ('szDescription', ctypes.c_char * 257),
                ('szSystemStatus', ctypes.c_char * 129), ('iMaxSockets', w.USHORT), ('iMaxUdpDg', w.USHORT),
                ('lpVendorInfo', ctypes.c_char_p)]


class GUID(ctypes.Structure):
    _fields_ = [('d1', w.DWORD), ('d2', w.WORD), ('d3', w.WORD), ('d4', ctypes.c_ubyte * 8)]


class SOCKADDR_BTH(ctypes.Structure):
    _pack_ = 1
    _fields_ = [('addressFamily', w.USHORT), ('btAddr', ctypes.c_ulonglong),
                ('serviceClassId', GUID), ('port', w.ULONG)]


class SYSTEMTIME(ctypes.Structure):
    _fields_ = [(n, w.WORD) for n in ('y', 'mo', 'dow', 'd', 'h', 'mi', 's', 'ms')]


class BLUETOOTH_DEVICE_INFO(ctypes.Structure):
    _fields_ = [('dwSize', w.DWORD), ('Address', ctypes.c_ulonglong), ('ulClassofDevice', w.ULONG),
                ('fConnected', w.BOOL), ('fRemembered', w.BOOL), ('fAuthenticated', w.BOOL),
                ('stLastSeen', SYSTEMTIME), ('stLastUsed', SYSTEMTIME), ('szName', w.WCHAR * 248)]


class BLUETOOTH_DEVICE_SEARCH_PARAMS(ctypes.Structure):
    _fields_ = [('dwSize', w.DWORD), ('fReturnAuthenticated', w.BOOL), ('fReturnRemembered', w.BOOL),
                ('fReturnUnknown', w.BOOL), ('fReturnConnected', w.BOOL), ('fIssueInquiry', w.BOOL),
                ('cTimeoutMultiplier', ctypes.c_ubyte), ('hRadio', w.HANDLE)]


ws.socket.restype = ctypes.c_void_p
ws.connect.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int]
ws.recv.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int, ctypes.c_int]
ws.setsockopt.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_char_p, ctypes.c_int]
ws.closesocket.argtypes = [ctypes.c_void_p]
bt.BluetoothFindFirstDevice.restype = ctypes.c_void_p
bt.BluetoothFindFirstDevice.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
bt.BluetoothFindNextDevice.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
bt.BluetoothFindDeviceClose.argtypes = [ctypes.c_void_p]
_wsa = WSADATA()
ws.WSAStartup(0x0202, ctypes.byref(_wsa))

ERR_TEXT = {
    10060: 'pad not reachable - is it on and blinking? (hold A + HOME)',
    10051: 'bond mismatch - remove "Gamesir-G4" in Windows Bluetooth settings, hold A + HOME, pair again',
    10053: 'connection aborted',
    10054: 'pad closed the connection',
    10064: 'pad does not answer (switched off?)',
}


def find_paired_g4():
    """Return the Bluetooth address of a paired device whose name looks like a G4, else None."""
    sp = BLUETOOTH_DEVICE_SEARCH_PARAMS(ctypes.sizeof(BLUETOOTH_DEVICE_SEARCH_PARAMS), 1, 1, 0, 1, 0, 0, None)
    di = BLUETOOTH_DEVICE_INFO(ctypes.sizeof(BLUETOOTH_DEVICE_INFO))
    h = bt.BluetoothFindFirstDevice(ctypes.byref(sp), ctypes.byref(di))
    if not h:
        return None
    found = None
    while True:
        if NAME_PATTERN.search(di.szName):
            found = di.Address
            break
        if not bt.BluetoothFindNextDevice(h, ctypes.byref(di)):
            break
    bt.BluetoothFindDeviceClose(h)
    return found


def parse_addr(text):
    hexdigits = re.sub(r'[^0-9A-Fa-f]', '', text)
    if len(hexdigits) != 12:
        raise argparse.ArgumentTypeError('address must look like AA:BB:CC:DD:EE:FF')
    return int(hexdigits, 16)


def fmt_addr(a):
    return ':'.join('%02X' % ((a >> (8 * i)) & 0xFF) for i in range(5, -1, -1))


def connect(addr):
    s = ws.socket(AF_BTH, SOCK_STREAM, BTHPROTO_RFCOMM)
    sa = SOCKADDR_BTH(AF_BTH, addr, GUID(), CHANNEL)
    if ws.connect(s, ctypes.byref(sa), ctypes.sizeof(sa)) != 0:
        e = ws.WSAGetLastError()
        ws.closesocket(s)
        return None, e
    ws.setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, struct.pack('i', 1000), 4)
    return s, 0


def reports(sock):
    """Yield (report_id, payload) from the RFCOMM stream. Returns when the link drops."""
    buf = bytearray()
    chunk = ctypes.create_string_buffer(512)
    while True:
        n = ws.recv(sock, chunk, 512, 0)
        if n == 0:
            return
        if n < 0:
            e = ws.WSAGetLastError()
            if e == 10060:          # receive timeout: the pad is idle, that is normal
                continue
            print('recv error %d: %s' % (e, ERR_TEXT.get(e, '')), flush=True)
            return
        buf += chunk.raw[:n]
        while buf:
            if buf[0] != 0xA1:      # resync on the DATA | Input header
                del buf[0]
                continue
            if len(buf) < 2:
                break
            ln = REPORT_LEN.get(buf[1])
            if ln is None:
                del buf[0]
                continue
            if len(buf) < ln:
                break
            yield buf[1], bytes(buf[2:ln])
            del buf[:ln]


def axis(v, invert=False):
    """8-bit HID axis (0..255, 128 = centre) -> XInput int16. Invert before clamping."""
    val = (v - 128) * 258
    if invert:
        val = -val
    return max(-32768, min(32767, val))


# hat switch value -> (dx, dy); 1 = north, clockwise; 0 / 15 = released
HAT = {1: (0, 1), 2: (1, 1), 3: (1, 0), 4: (1, -1), 5: (0, -1), 6: (-1, -1), 7: (-1, 0), 8: (-1, 1)}


def wait_for_pad(addr, quiet=False):
    last_err = None
    while True:
        s, e = connect(addr)
        if s is not None:
            return s
        if e != last_err and not quiet:
            print('%s  WSA %d: %s' % (time.strftime('%H:%M:%S'), e, ERR_TEXT.get(e, '')), flush=True)
            last_err = e
        time.sleep(1)


def run(addr):
    import vgamepad as vg
    B = vg.XUSB_BUTTON
    xb = {'A': B.XUSB_GAMEPAD_A, 'B': B.XUSB_GAMEPAD_B, 'X': B.XUSB_GAMEPAD_X, 'Y': B.XUSB_GAMEPAD_Y,
          'LB': B.XUSB_GAMEPAD_LEFT_SHOULDER, 'RB': B.XUSB_GAMEPAD_RIGHT_SHOULDER,
          'BACK': B.XUSB_GAMEPAD_BACK, 'START': B.XUSB_GAMEPAD_START, 'GUIDE': B.XUSB_GAMEPAD_GUIDE,
          'L3': B.XUSB_GAMEPAD_LEFT_THUMB, 'R3': B.XUSB_GAMEPAD_RIGHT_THUMB}
    mapping = dict(DEFAULT_MAP)
    if os.path.exists(MAP_FILE):
        with open(MAP_FILE) as f:
            mapping.update(json.load(f))
    pad = vg.VX360Gamepad()
    print('virtual Xbox 360 controller created; waiting for %s ...' % fmt_addr(addr), flush=True)
    while True:
        s = wait_for_pad(addr)
        print('%s  CONNECTED' % time.strftime('%H:%M:%S'), flush=True)
        for rid, p in reports(s):
            if rid != 0xC4 or len(p) < 9:
                continue
            x, y, z, rz, lt, rt = p[0], p[1], p[2], p[3], p[4], p[5]
            btn = p[6] | (p[7] << 8)
            hat = p[8] & 0x0F
            pad.left_joystick(axis(x), axis(y, True))
            pad.right_joystick(axis(z), axis(rz, True))
            pad.left_trigger(lt)
            pad.right_trigger(rt)
            for name, bit in mapping.items():
                (pad.press_button if btn & (1 << bit) else pad.release_button)(xb[name])
            dx, dy = HAT.get(hat, (0, 0))
            (pad.press_button if dy > 0 else pad.release_button)(B.XUSB_GAMEPAD_DPAD_UP)
            (pad.press_button if dy < 0 else pad.release_button)(B.XUSB_GAMEPAD_DPAD_DOWN)
            (pad.press_button if dx < 0 else pad.release_button)(B.XUSB_GAMEPAD_DPAD_LEFT)
            (pad.press_button if dx > 0 else pad.release_button)(B.XUSB_GAMEPAD_DPAD_RIGHT)
            pad.update()
        ws.closesocket(s)
        pad.reset()
        pad.update()
        print('%s  link lost, reconnecting ...' % time.strftime('%H:%M:%S'), flush=True)


def calibrate(addr):
    print('calibration - waiting for the pad ...', flush=True)
    s = wait_for_pad(addr)
    print('CONNECTED', flush=True)
    gen = reports(s)
    mapping = {}
    for name in ['A', 'B', 'X', 'Y', 'LB', 'RB', 'BACK', 'START', 'GUIDE', 'L3', 'R3']:
        print('press: %s' % name, flush=True)
        deadline = time.time() + 8
        got = None
        while time.time() < deadline and got is None:
            for rid, p in gen:
                if rid == 0xC4:
                    btn = p[6] | (p[7] << 8)
                    if btn:
                        got = (btn & -btn).bit_length() - 1
                        break
                if time.time() > deadline:
                    break
        if got is None:
            print('  skipped', flush=True)
            continue
        mapping[name] = got
        print('  %s -> bit %d' % (name, got), flush=True)
        time.sleep(0.6)
        for rid, p in gen:                    # wait until released
            if rid == 0xC4 and (p[6] | (p[7] << 8)) == 0:
                break
    with open(MAP_FILE, 'w') as f:
        json.dump(mapping, f, indent=2)
    print('saved:', MAP_FILE, flush=True)
    ws.closesocket(s)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('mode', nargs='?', choices=['run', 'calibrate'], default='run')
    ap.add_argument('--addr', type=parse_addr, help='Bluetooth address of the pad '
                    '(default: auto-detect a paired "Gamesir-G4")')
    ap.add_argument('--version', action='version', version='g4bridge ' + __version__)
    args = ap.parse_args()
    addr = args.addr or find_paired_g4()
    if addr is None:
        sys.exit('no paired Gamesir-G4 found. Pair it in Windows Bluetooth settings first (hold A + HOME), '
                 'or pass --addr XX:XX:XX:XX:XX:XX')
    try:
        (calibrate if args.mode == 'calibrate' else run)(addr)
    except KeyboardInterrupt:
        pass


if __name__ == '__main__':
    main()
