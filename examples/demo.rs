//! Live demo: opens the default mic, prints a roll of `randint(1, 6)` once
//! per second along with the detector state. Runs until Ctrl+C.
//!
//! The indicator on the right surfaces the current monitor state. Try
//! running it under different ambient acoustic conditions.

use std::io::Write;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn state_label(triggered: bool) -> &'static str {
    if triggered {
        "TRIGGER"
    } else {
        "  --   "
    }
}

fn wall_clock_hms() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let day_secs = secs % 86_400;
    let h = day_secs / 3600;
    let m = (day_secs % 3600) / 60;
    let s = day_secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn main() {
    // Monitoring is opt-in: hoba only spawns its background detector when
    // HOBA_MONITOR=1 is set in the process environment. The demo opts itself
    // in here so the `monitor` column updates without extra setup.
    // SAFETY: single-threaded program startup; no other threads observe env.
    std::env::set_var("HOBA_MONITOR", "1");

    println!("hoba demo — randint(1, 6) once per second.");
    println!("Watch the `monitor` column as the room conditions change.");
    #[cfg(not(feature = "mic"))]
    println!("(built without `mic` feature; the monitor stays disabled)");
    println!();

    loop {
        let roll = hoba::randint(1, 6);
        let triggered = hoba::is_compromised();
        println!(
            "{}  roll={roll}  monitor={}",
            wall_clock_hms(),
            state_label(triggered)
        );
        std::io::stdout().flush().ok();
        std::thread::sleep(Duration::from_secs(1));
    }
}
