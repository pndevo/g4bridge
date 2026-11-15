# G4Bridge

[![CI](https://github.com/pndevo/g4bridge/actions/workflows/ci.yml/badge.svg)](https://github.com/pndevo/g4bridge/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platform: Windows](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-lightgrey)

Use a **GameSir G4** gamepad over **Bluetooth on Windows** - sticks, triggers, D-pad and all buttons - even though Windows refuses to treat the pad as a gamepad.

The G4 (the original model, not the G4s / G4 Pro) is sold as an Android/iOS Bluetooth controller. On Windows it pairs, then sits there blinking forever: Windows reads the pad's Bluetooth HID record, finds it non-compliant, and silently never installs the gamepad driver (details in [FINDINGS.md](FINDINGS.md)). The firmware, however, also streams the very same gamepad reports over a Bluetooth **serial channel** (RFCOMM channel 1, service name `WirelessCom`), and any user-mode program may open that. G4Bridge opens it, parses the reports and drives a virtual Xbox 360 controller through [ViGEmBus](https://github.com/nefarius/ViGEmBus). Games see a normal XInput pad.

G4Bridge itself needs no driver of its own, no admin rights and no firmware flashing. The only thing to install is the ViGEmBus driver (one time, installer asks for admin), which most gamepad tools such as DS4Windows already put on the system.

## Requirements

- **Windows 10 / 11** with a Bluetooth adapter (classic Bluetooth, not BLE-only) and the [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases) driver (DS4Windows users already have it)
- or **Linux** with BlueZ and `/dev/uinput` access (experimental, see below)

## Install

Download `g4bridge-windows-x64.exe` from the [latest release](https://github.com/pndevo/g4bridge/releases/latest) and run it. Nothing else to install. The file is not code-signed, so Windows SmartScreen may warn once: *More info* → *Run anyway*. Compare the SHA256 file from the release if you want to be sure.

Build from source (Rust 1.75+): `cargo build --release`, binary in `target/release/`.

A Python version with the same behaviour lives in [`python/`](python/) for people who prefer a script (`pip install vgamepad`, then `python g4bridge.py`).

## Setup

1. Switch the pad off. Hold **A + HOME** for 2 s - the HOME LED blinks (Android Bluetooth mode).
2. Windows Settings → Bluetooth → *Add device* → `Gamesir-G4`. It pairs, then keeps blinking. That is expected.
3. Run `g4bridge.exe`. Within a few seconds the console prints `CONNECTED` and the LED stays solid.
4. Check with `joy.cpl` or any gamepad tester: an *Xbox 360 Controller* shows up.

Keep the console window open while you play. Closing it drops the link and the pad goes back to blinking.

## Every day use

- Start `g4bridge.exe` first, then switch the pad on with **A + HOME**. The bridge reconnects automatically whenever the pad reappears, also after the pad powered itself off.
- If the console says `bond mismatch`: the pad forgot the pairing (it does that when you re-enter pairing mode). Remove `Gamesir-G4` in Windows Bluetooth settings and pair it again - the running bridge picks it up on its own.
- Wired use is unaffected: plug in the cable, hold **X + HOME**, the pad is a native Xbox 360 controller.

## Button layout

Sticks, triggers (analog) and the D-pad come straight from the report and need no mapping. The 16 button bits follow the Android gamepad layout, which is the default and was verified on a real G4:

| bit | button | bit | button |
|----:|--------|----:|--------|
| 0 | A | 8 | LT pressed (digital copy) |
| 1 | B | 9 | RT pressed (digital copy) |
| 3 | X | 10 | Select |
| 4 | Y | 11 | Start |
| 6 | LB | 12 | HOME |
| 7 | RB | 13 / 14 | L3 / R3 |

Should your unit differ, run `g4bridge.exe calibrate`, press the buttons as prompted, and restart the bridge. The result is stored in `bt_map.json` next to the program.

## Options

```
g4bridge.exe                      run (default), auto-detects a paired "Gamesir-G4"
g4bridge.exe calibrate            learn button bits
g4bridge.exe --addr AA:BB:CC:DD:EE:FF
                                  use a specific Bluetooth address
g4bridge.exe --version
```

## How it works

```
GameSir G4 ──Bluetooth RFCOMM ch.1──▶ g4bridge ──ViGEmBus──▶ virtual Xbox 360 pad ──XInput──▶ game
```

Every packet on the serial channel is a Bluetooth HID input report, header byte `0xA1` followed by the report ID:

| ID | length | content |
|----|-------:|---------|
| `0xC4` | 12 | gamepad: X, Y, Z, Rz (8 bit, 128 = centre, Y 0 = up), brake = LT, accelerator = RT, 16 button bits (little endian), hat nibble (1 = N … 8 = NW, 0 = released) |
| `0x12` | 5 | consumer control (media keys) - ignored |
| `0x06` | 6 | mouse - ignored |

The bridge resynchronises on `0xA1`, so partial packets are harmless. The pad only sends when something changes; a 1 s receive timeout therefore means *idle*, not *gone*. A closed socket or `WSAECONNRESET` means the pad is gone, and the bridge resets the virtual pad and goes back to retrying once a second.

## Linux (experimental)

A Linux build exists (BlueZ RFCOMM socket → virtual gamepad via `uinput`), but it has **not been tested on real hardware**. BlueZ may well accept the pad as a normal HID gamepad already - try that first. If not:

```
bluetoothctl            # pair the pad (A + HOME), then quit
sudo modprobe uinput
sudo setfacl -m u:$USER:rw /dev/uinput
./g4bridge-linux-x64
```

Reports of it working or failing are welcome.

## Troubleshooting

| console says | meaning / fix |
|--------------|---------------|
| `no paired Gamesir-G4 found` | pair the pad in Windows Bluetooth settings first (A + HOME), or pass `--addr` |
| `WSA 10060: pad not reachable` | pad is off or not in A + HOME mode; the bridge keeps retrying |
| `WSA 10051: bond mismatch` | remove the pad in Windows Bluetooth settings and pair again |
| `cannot talk to ViGEmBus` | install the [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases) driver |

## Known limitations

- One pad per bridge instance. Several instances with `--addr` should work but were not tested.
- No rumble: the G4 has none.
- Tested with one G4 (USB ID `05AC:2405`, firmware 2.13, Bluetooth `0A5C:0239`) on Windows 11 25H2 with a MediaTek adapter. Other firmware revisions may use another channel or report layout; `FINDINGS.md` explains how to check.

## Disclaimer

Not affiliated with or endorsed by GameSir, Microsoft or the ViGEm project. Use at your own risk. ViGEmBus is no longer actively developed upstream but still works on current Windows 11.

## Contributing

Reports from other firmware revisions and adapters are very welcome - see [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT - see [LICENSE](LICENSE).
