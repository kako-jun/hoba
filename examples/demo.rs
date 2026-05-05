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
    println!("hoba demo — randint(1, 6) once per second.");
    println!("Tip: play an ultrasonic tone (19.0–20.5 kHz, 0.5 kHz buckets) near the mic.");
    println!("State legend: `  --  ` no trigger;  `19.0k!`–`20.5k!` bucket detected (depth 1..4).");
    println!("When triggered, rolls collapse to {{1, 3, 5}} (LSB cleared).");
    #[cfg(not(feature = "mic"))]
    println!("(built without `mic` feature; trigger detection disabled, depth stays 0)");
    println!();

    loop {
        let roll = hoba::randint(1, 6);
        let depth = hoba::compromised_depth();
        println!(
            "{}  roll={roll}  detector={}",
            wall_clock_hms(),
            bucket_label(depth)
        );
        std::io::stdout().flush().ok();
        std::thread::sleep(Duration::from_secs(1));
    }
}
