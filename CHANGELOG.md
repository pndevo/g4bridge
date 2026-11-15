# Changelog

## 0.1.0

- Native Rust build (single `.exe` for Windows, experimental Linux binary via `uinput`); Python version kept in `python/`.
- First public release: GameSir G4 over Bluetooth on Windows via RFCOMM channel 1 and a ViGEm virtual Xbox 360 controller.
- Auto-detection of a paired `Gamesir-G4`, automatic reconnect, `calibrate` mode for the button bits.
