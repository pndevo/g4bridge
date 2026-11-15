# Contributing

Bug reports with the console output, pad firmware and Bluetooth adapter are the most useful contribution - especially from other G4 firmware revisions (see the end of [FINDINGS.md](FINDINGS.md)).

Pull requests: keep the tool small and dependency-light. Before opening one, run

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

(For the Python version in `python/`: `ruff check .` and `python -m pytest` inside that folder.)

The tests cover the pure helpers only; talking to a real pad needs the hardware.
