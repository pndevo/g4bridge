# Why the GameSir G4 never works as a Bluetooth gamepad on Windows

Investigation done on 2026-10-10 on Windows 11 25H2 (build 26200), MediaTek Bluetooth adapter (USB `0489:E0E2`), GameSir G4 firmware 2.13. Tools: `Get-PnpDevice`, the Windows Bluetooth API (`bthprops.cpl`), an ETW trace of `Microsoft-Windows-BTH-BTHPORT` / `BTHUSB`, and the SDP records Windows caches under `HKLM\SYSTEM\CurrentControlSet\Services\BTHPORT\Parameters\Devices\<addr>\CachedServices`.

## Symptom

- Pairing in Windows succeeds (`Gamesir-G4` appears, BTHUSB event 8 "connected").
- The HOME LED keeps blinking; Windows shows *paired, not connected*.
- Device Manager has only the generic node `BTHENUM\DEV_<addr>` (driver `bth.inf`, `BthGenericDevice`). The HID child `BTHENUM\{00001124-...}` that every working Bluetooth HID device gets (tested against a DualSense and a Keychron K6 on the same adapter) is never created.
- `BluetoothEnumerateInstalledServices` returns 0 services; `BluetoothSetServiceState(HID)` fails with `ERROR_INVALID_PARAMETER` (87). Creating the service keys by hand under `ServicesFor<radio>\{00001124-...}` and restarting the node changes nothing.

## What the pad does in each mode

| mode | Bluetooth class | what Windows gets |
|------|-----------------|-------------------|
| **A + HOME** (Android) | gamepad `0x2508` at pairing, keyboard `0x2540` in inquiry responses | SDP: Serial Port `0x1101` "WirelessCom" (RFCOMM ch 1), HID `0x1124` with a 268-byte report descriptor (consumer + **mouse** + gamepad), PnP `FFFF:046F` |
| **B + HOME** (Android 2) | gamepad `0x2508` | SDP: HID with a 204-byte descriptor (consumer + gamepad), PnP `05AC:022D` (Apple VID) |
| **Y + HOME** (iOS) | keyboard `0x2540` | connects as a plain **keyboard** (`0A5C:0239`). Buttons send single keys; sticks and triggers produce garbage keystrokes |
| **X + HOME** + USB cable | - | XInput, works natively as "Xbox 360 Controller for Windows" |

The pad also advertises over BLE as `Gamesir-G4GCM` (random address `AA:BB:CC:DD:EE:00`, flags `0x00`, a bogus 16-bit UUID list made of its own address bytes). It is not connectable and not pairable from Windows; it is only a beacon for the vendor's phone app.

## HCI timeline of one connection attempt (A + HOME, after pairing)

```
 6.5 s  Remote Name Request          OK ("Gamesir-G4")
27.7 s  Create Connection            OK, handle 0x34, role switch to slave
28.1 s  Link Key Request             host answers NEGATIVE -> full re-pair
        SSP, IO caps NoInputNoOutput -> Just Works, unauthenticated P-192 key
28.7 s  Encryption on
28.7 -  SDP ServiceSearchAttribute   all three records fetched, L2CAP channel closed cleanly
29.0 s
32.0 s  Disconnect, reason 0x13      *** initiated by Windows ***
```

Nobody ever opens L2CAP PSM `0x11` (HID control) or `0x13` (HID interrupt). Windows reads the records, decides there is nothing it wants, and drops the ACL link. The pad, with `HIDReconnectInitiate = true` and `HIDNormallyConnectable = false`, then waits for a reconnect that never comes - and after a minute or two switches itself off.

## What is wrong with the HID record

Decoded from the cached SDP record (Bluetooth HID Profile 1.0 attribute names):

| attribute | value | problem |
|-----------|-------|---------|
| `HIDDeviceSubclass` | `0x80` | means *pointing device*; the descriptor is a gamepad |
| `HIDBootDevice` | `true` | claims boot-protocol support, but there is no boot keyboard/mouse collection |
| `HIDVirtualCable` | `true` | fine |
| `HIDReconnectInitiate` / `HIDNormallyConnectable` | `true` / `false` | the pad expects to initiate the reconnect, but never does |
| SDP service record handles | `0x00000001`, `0x00000002`, `0x00000003` | handles `0x00000001 - 0x0000FFFF` are reserved by the Core spec; compliant devices (DualSense: `0x00010001 ff.`) use `0x00010000+` |
| PnP `VendorID` / `ProductID` | `FFFF:046F` or `05AC:022D` or `0A5C:0239` | changes with the mode; `05AC` is Apple, `0A5C` Broadcom |
| report descriptor, every axis | `15 00 25 FF` | 1-byte *Logical Maximum* `0xFF` is **-1** when parsed as signed; it should be `26 FF 00`. Windows' `hidparse` sees a range of 0 … -1 |
| hat switch | `46 3B 10` with unit `0x0E` | *Physical Maximum* 4155° (byte-swapped 315) and a reserved unit nibble |

Any one of the first two rows is enough for `bthport.sys` to refuse the device; the `Logical Maximum` bug would break the axes even if a HID node were created. Android and iOS are far more forgiving, which is why the pad works there.

None of this can be fixed from the PC side: it is the pad's firmware, there is no firmware update for the G4, and Windows offers no switch to bypass its HID validation.

## The way out: the serial channel

The A + HOME record set contains a Serial Port Profile record, RFCOMM channel 1, named `WirelessCom`. Opening it from user mode with Winsock (`AF_BTH`, `BTHPROTO_RFCOMM`, port 1) succeeds as soon as the pad is bonded and reachable. What arrives is the raw HID transport: `0xA1` (DATA | Input) followed by the report, exactly as it would arrive on the HID interrupt channel.

```
a1 c4 80 80 80 80 00 00 00 00 00 00      idle: all axes centred, no buttons
a1 c4 80 83 80 80 00 00 00 00 00 00      left stick moves
a1 c4 80 80 80 80 ff 00 00 01 00 00      LT fully pressed, bit 8 set
```

Layout of report `0xC4` (from the descriptor, 11 bytes after the ID):

| byte | content |
|-----:|---------|
| 0, 1 | left stick X, Y (0 … 255, 128 centre, Y = 0 is up) |
| 2, 3 | right stick Z, Rz |
| 4 | brake = left trigger, 0 … 255 |
| 5 | accelerator = right trigger, 0 … 255 |
| 6, 7 | 16 buttons, little endian; bits 8 / 9 mirror the triggers digitally |
| 8 | low nibble hat switch 1 … 8 clockwise from north, 0 = released |
| 9 | padding |

The pad holds the RFCOMM link open as long as the host does, sends only on change, and switches itself off some time after the link is dropped. `g4bridge.py` keeps the link open and maps the report onto a ViGEm Xbox 360 pad.

## Things that do not help (tested)

- Forcing the HID service via `BluetoothSetServiceState` or registry keys.
- Deleting and re-pairing in any of the three Bluetooth modes.
- Pairing the BLE beacon (`CanPair = false`, GATT `Unreachable`).
- Mapping the Y + HOME keyboard mode: no triggers, no sticks, two D-pad directions share a key.

## Things that were not tested

- Another Bluetooth adapter chipset (unlikely to matter: Windows drops the link after a clean SDP exchange).
- Other G4 firmware revisions. If yours shows no `WirelessCom` record, look at `CachedServices` for the address and search the hex for `19 11 01` (Serial Port UUID) and `19 11 24` (HID UUID).
