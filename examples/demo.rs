//! Live demo: opens the default mic, prints a roll of `randint(1, 6)` once
//! per second along with the detector state. Runs until Ctrl+C.
//!
//! Try playing an ultrasonic tone (19.0 / 19.5 / 20.0 / 20.5 kHz) near the
//! mic. The indicator on the right surfaces which 0.5 kHz bucket is dominant
//! and how many low bits get cleared from `random_u64`.

use std::io::Write;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn bucket_label(depth: u8) -> &'static str {
    match depth {
        0 => "  --  ",
        1 => "19.0k!",
        2 => "19.5k!",
        3 => "20.0k!",
        4 => "20.5k!",
        _ => "  ?   ",
    }
}

fn main() {
    println!("hoba demo — randint(1, 6) once per second.");
    println!("Tip: play an ultrasonic tone (19.0–20.5 kHz, 0.5 kHz buckets) near the mic.");
    println!("State legend: `  --  ` no trigger;  `19.0k!`–`20.5k!` bucket detected (depth 1..4).");
    println!();

    loop {
        let roll = hoba::randint(1, 6);
        let depth = hoba::compromised_depth();
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        println!("t={ts}  roll={roll}  detector={}", bucket_label(depth));
        std::io::stdout().flush().ok();
        std::thread::sleep(Duration::from_secs(1));
    }
}
