//! G4Bridge - use a GameSir G4 gamepad over Bluetooth.
//!
//! Windows never installs the HID service for the G4 (non-compliant SDP record, see FINDINGS.md), but the
//! firmware also streams the same input reports over a Bluetooth serial channel (RFCOMM channel 1). This
//! program reads that channel and drives a virtual Xbox 360 controller.

mod platform;
mod report;

use platform::{Addr, Link, Recv};
use report::{Mapping, PadReport, Parser, BUTTONS};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use std::{env, fs, process, thread};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const USAGE: &str = "\
g4bridge - GameSir G4 over Bluetooth

USAGE:
    g4bridge [run]               run; reconnects forever, Ctrl+C stops
    g4bridge calibrate           learn the button bits, writes bt_map.json
    g4bridge --addr AA:BB:CC:DD:EE:FF
                                 skip auto-detection, use this Bluetooth address
    g4bridge --version | --help";

fn fmt_addr(a: Addr) -> String {
    (0..6).rev().map(|i| format!("{:02X}", (a >> (8 * i)) & 0xFF)).collect::<Vec<_>>().join(":")
}

fn parse_addr(text: &str) -> Result<Addr, String> {
    let digits: String = text.chars().filter(char::is_ascii_hexdigit).collect();
    if digits.len() != 12 {
        return Err("address must look like AA:BB:CC:DD:EE:FF".into());
    }
    u64::from_str_radix(&digits, 16).map_err(|e| e.to_string())
}

fn map_path() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("bt_map.json")))
        .unwrap_or_else(|| PathBuf::from("bt_map.json"))
}

fn load_mapping() -> Mapping {
    match fs::read_to_string(map_path()) {
        Ok(text) => Mapping::from_json(&text).unwrap_or_else(|e| {
            eprintln!("ignoring bt_map.json: {e}");
            Mapping::default()
        }),
        Err(_) => Mapping::default(),
    }
}

struct Clock(Instant);

impl Clock {
    fn stamp(&self) -> String {
        format!("[{:>5}s]", self.0.elapsed().as_secs())
    }
}

/// Retry once a second until the pad answers.
fn wait_for_pad(addr: Addr, clock: &Clock) -> Box<dyn Link> {
    let mut last_err = None;
    loop {
        match platform::connect_pad(addr) {
            Ok(link) => return link,
            Err(e) => {
                if last_err != Some(e) {
                    println!("{} error {}: {}", clock.stamp(), e, platform::describe_error(e));
                    last_err = Some(e);
                }
                thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

fn run(addr: Addr) -> Result<(), String> {
    let mapping = load_mapping();
    let mut pad = platform::open_pad()?;
    let clock = Clock(Instant::now());
    println!("virtual Xbox 360 controller created; waiting for {} ...", fmt_addr(addr));
    let mut buf = [0u8; 512];
    loop {
        let mut link = wait_for_pad(addr, &clock);
        println!("{} CONNECTED", clock.stamp());
        let mut parser = Parser::default();
        loop {
            match link.recv(&mut buf) {
                Recv::Idle => {}
                Recv::Data(n) => {
                    for packet in parser.push(&buf[..n]) {
                        if let Some(r) = PadReport::from_packet(&packet) {
                            pad.update(&mapping.apply(&r));
                        }
                    }
                }
                Recv::Closed(code) => {
                    if let Some(c) = code {
                        println!("{} recv error {}: {}", clock.stamp(), c, platform::describe_error(c));
                    }
                    break;
                }
            }
        }
        drop(link);
        pad.reset();
        println!("{} link lost, reconnecting ...", clock.stamp());
    }
}

/// Wait up to `timeout` for a gamepad report satisfying `want`.
fn next_report(
    link: &mut Box<dyn Link>,
    parser: &mut Parser,
    timeout: Duration,
    want: impl Fn(&PadReport) -> bool,
) -> Option<PadReport> {
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 512];
    while Instant::now() < deadline {
        match link.recv(&mut buf) {
            Recv::Idle => {}
            Recv::Closed(_) => return None,
            Recv::Data(n) => {
                for packet in parser.push(&buf[..n]) {
                    if let Some(r) = PadReport::from_packet(&packet) {
                        if want(&r) {
                            return Some(r);
                        }
                    }
                }
            }
        }
    }
    None
}

fn calibrate(addr: Addr) -> Result<(), String> {
    println!("calibration - waiting for the pad ...");
    let clock = Clock(Instant::now());
    let mut link = wait_for_pad(addr, &clock);
    println!("CONNECTED");
    let mut parser = Parser::default();
    let mut mapping = Mapping(Default::default());
    for (name, _, _) in BUTTONS {
        println!("press: {name}");
        match next_report(&mut link, &mut parser, Duration::from_secs(8), |r| r.buttons != 0) {
            Some(r) => {
                let bit = r.buttons.trailing_zeros() as u8;
                println!("  {name} -> bit {bit}");
                mapping.0.insert(name.to_string(), bit);
                thread::sleep(Duration::from_millis(600));
                next_report(&mut link, &mut parser, Duration::from_secs(10), |r| r.buttons == 0);
            }
            None => println!("  skipped"),
        }
    }
    let path = map_path();
    let json = serde_json::to_string_pretty(&mapping.0).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    println!("saved: {}", path.display());
    Ok(())
}

fn real_main() -> Result<(), String> {
    let mut mode = "run".to_string();
    let mut addr = None;
    let mut args = env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "run" | "calibrate" => mode = a,
            "--addr" => addr = Some(parse_addr(&args.next().ok_or("--addr needs a value")?)?),
            "--version" | "-V" => {
                println!("g4bridge {VERSION}");
                return Ok(());
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown argument '{other}'\n\n{USAGE}")),
        }
    }
    let addr = addr.or_else(platform::find_paired).ok_or(
        "no paired Gamesir-G4 found. Pair it in the Bluetooth settings first (hold A + HOME), \
         or pass --addr XX:XX:XX:XX:XX:XX",
    )?;
    if mode == "calibrate" {
        calibrate(addr)
    } else {
        run(addr)
    }
}

fn main() {
    if let Err(msg) = real_main() {
        eprintln!("error: {msg}");
        // A double-clicked console window would vanish before the message can be read.
        if env::args().len() == 1 {
            eprintln!("\nPress Enter to close.");
            let _ = std::io::stdin().read_line(&mut String::new());
        }
        process::exit(1);
    }
}
