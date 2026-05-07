//! `hoba` command-line tool. Reads the shared event log written by
//! hoba-using processes when `HOBA_MONITOR=1` is set.

use std::io::{self, Write};
use std::time::Duration;

use clap::{Parser, Subcommand};
use hoba::check::{self, format_results, BandResult, DEFAULT_BANDS_HZ, DEFAULT_DURATION_SECS};
use hoba::Event;
use time::format_description::well_known::Iso8601;
use time::OffsetDateTime;

#[derive(Parser)]
#[command(name = "hoba", about = "Inspect the hoba event log")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List recent events from the log, newest first.
    Log {
        /// Filter by time window. Relative: `30s`, `10m`, `2h`, `1d`. Absolute: ISO8601 (e.g. `2026-05-02T13:00:00Z`).
        #[arg(long, value_parser = parse_since_str)]
        since: Option<OffsetDateTime>,
        /// Output raw JSONL instead of human-readable lines.
        #[arg(long)]
        json: bool,
    },
    /// Live-tail: print new events as they are appended to the log.
    Watch,
    /// Self-test: emit a tone (or listen for one) and report whether each
    /// target band clears the SNR threshold at the mic — i.e. whether the
    /// production trigger would fire on this device.
    Check {
        /// Comma-separated frequencies in Hz to probe (e.g. `19000,19500`).
        /// For back-compat with v0.4.x, items may carry a `:depth` suffix
        /// (`19000:1,19500:2`) — the `:depth` portion is silently dropped
        /// because the graded depth concept was removed in v0.5.0.
        #[arg(long)]
        bands: Option<String>,
        /// Per-band measurement duration in seconds.
        #[arg(long, default_value_t = DEFAULT_DURATION_SECS)]
        duration: u64,
        /// Override the SNR threshold (in dB) the per-band verdict applies.
        /// Default: 6 dB, matching `DetectorConfig::snr_threshold_db`.
        /// Tighten to 10–12 dB for noisy rooms; loosen to 3 dB for a
        /// hair-trigger. `--threshold` is kept as a deprecated alias for
        /// `--snr` and now also takes a dB SNR value (no longer the v0.3.x
        /// raw-power knob).
        #[arg(long, alias = "threshold")]
        snr: Option<f32>,
        /// Do not emit a tone; measure only. Use an external source.
        #[arg(long)]
        listen_only: bool,
        /// cpal input device name (defaults to host default).
        #[arg(long)]
        input_device: Option<String>,
        /// cpal output device name (defaults to host default).
        #[arg(long)]
        output_device: Option<String>,
        /// Print available input/output devices and exit.
        #[arg(long)]
        list_devices: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Log { since, json } => cmd_log(since, json),
        Cmd::Watch => cmd_watch(),
        Cmd::Check {
            bands,
            duration,
            snr,
            listen_only,
            input_device,
            output_device,
            list_devices,
        } => {
            let exit = cmd_check(
                bands,
                duration,
                snr,
                listen_only,
                input_device,
                output_device,
                list_devices,
            );
            std::process::exit(exit);
        }
    }
}

fn cmd_log(since: Option<OffsetDateTime>, json: bool) {
    let now = OffsetDateTime::now_utc();
    let cutoff = since.unwrap_or_else(|| now - time::Duration::days(30));
    let window = (now - cutoff).whole_seconds().max(0) as u64;
    let mut events = hoba::recent_events(Duration::from_secs(window));
    // Filter strictly against cutoff (recent_events uses a window, but the user's --since
    // could be more precise than the seconds-resolution window).
    events.retain(|e| {
        OffsetDateTime::parse(&e.ts, &Iso8601::DEFAULT)
            .map(|t| t >= cutoff)
            .unwrap_or(false)
    });
    // Newest first
    events.sort_by(|a, b| b.ts.cmp(&a.ts));
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for event in &events {
        if json {
            if let Ok(s) = serde_json::to_string(event) {
                let _ = writeln!(out, "{s}");
            }
        } else {
            let _ = writeln!(out, "{}", format_event(event));
        }
    }
}

fn cmd_watch() {
    let mut last_ts: Option<String> = None;
    let stdout = io::stdout();
    loop {
        let events = hoba::recent_events(Duration::from_secs(86_400)); // last 24h window
        let mut out = stdout.lock();
        for event in &events {
            // MSRV 1.78: keep `map_or` instead of `is_none_or` (1.82+).
            if last_ts.as_ref().map_or(true, |prev| event.ts > *prev) {
                let _ = writeln!(out, "{}", format_event(event));
                let _ = out.flush();
            }
        }
        if let Some(latest) = events.iter().map(|e| &e.ts).max() {
            last_ts = Some(latest.clone());
        }
        drop(out);
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn parse_since(s: &str) -> Option<OffsetDateTime> {
    // Try absolute first
    if let Ok(dt) = OffsetDateTime::parse(s, &Iso8601::DEFAULT) {
        return Some(dt);
    }
    // Relative: number followed by single-letter unit
    if s.len() < 2 {
        return None;
    }
    let (num, unit) = s.split_at(s.len() - 1);
    let n: i64 = num.parse().ok()?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86_400,
        _ => return None,
    };
    Some(OffsetDateTime::now_utc() - time::Duration::seconds(secs))
}

/// clap value parser wrapper that turns a parse failure into a user-visible error
/// (clap then exits non-zero with a hint), instead of silently falling back.
fn parse_since_str(s: &str) -> Result<OffsetDateTime, String> {
    parse_since(s).ok_or_else(|| {
        format!(
            "could not parse '{s}' as a time. Use a relative window like '30s', '10m', '2h', '1d', or an ISO8601 timestamp like '2026-05-02T13:00:00Z'."
        )
    })
}

fn cmd_check(
    bands: Option<String>,
    duration_secs: u64,
    snr: Option<f32>,
    listen_only: bool,
    input_device: Option<String>,
    output_device: Option<String>,
    list_devices: bool,
) -> i32 {
    if list_devices {
        list_audio_devices();
        return 0;
    }
    let bands = match bands {
        Some(s) => match check::parse_bands(&s) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("hoba check: --bands: {e}");
                return 2;
            }
        },
        None => DEFAULT_BANDS_HZ.to_vec(),
    };
    if let Some(t) = snr {
        if !t.is_finite() || t < 0.0 {
            eprintln!("hoba check: --snr must be non-negative and finite, got {t}");
            return 2;
        }
    }
    let opts = check::CheckOptions {
        bands,
        duration: Duration::from_secs(duration_secs.max(1)),
        listen_only,
        input_device,
        output_device,
        snr_threshold_db: snr,
    };
    let results: Vec<BandResult> = match check::run_check(&opts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("hoba check: {e}");
            return 1;
        }
    };
    let table = format_results(&results);
    let stdout = io::stdout();
    let _ = stdout.lock().write_all(table.as_bytes());
    if check::decide_verdict(&results) {
        0
    } else {
        1
    }
}

fn list_audio_devices() {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let default_input = host
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_else(|| "(none)".into());
    let default_output = host
        .default_output_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_else(|| "(none)".into());
    println!("host: {}", host.id().name());
    println!("default input : {default_input}");
    println!("default output: {default_output}\n");

    println!("--- inputs ---");
    match host.input_devices() {
        Ok(it) => {
            for (i, dev) in it.enumerate() {
                let name = dev.name().unwrap_or_else(|_| "?".into());
                let cfg = dev
                    .default_input_config()
                    .map(|c| {
                        format!(
                            "{} ch, {} Hz, {:?}",
                            c.channels(),
                            c.sample_rate().0,
                            c.sample_format()
                        )
                    })
                    .unwrap_or_else(|e| format!("(no default config: {e})"));
                let mark = if name == default_input { " *" } else { "  " };
                println!("[{i}]{mark} {name}  | {cfg}");
            }
        }
        Err(e) => println!("(input enumeration failed: {e})"),
    }

    println!("\n--- outputs ---");
    match host.output_devices() {
        Ok(it) => {
            for (i, dev) in it.enumerate() {
                let name = dev.name().unwrap_or_else(|_| "?".into());
                let cfg = dev
                    .default_output_config()
                    .map(|c| {
                        format!(
                            "{} ch, {} Hz, {:?}",
                            c.channels(),
                            c.sample_rate().0,
                            c.sample_format()
                        )
                    })
                    .unwrap_or_else(|e| format!("(no default config: {e})"));
                let mark = if name == default_output { " *" } else { "  " };
                println!("[{i}]{mark} {name}  | {cfg}");
            }
        }
        Err(e) => println!("(output enumeration failed: {e})"),
    }
}

fn format_event(e: &Event) -> String {
    let time = OffsetDateTime::parse(&e.ts, &Iso8601::DEFAULT)
        .map(|t| {
            let h = t.hour();
            let m = t.minute();
            let s = t.second();
            format!("{h:02}:{m:02}:{s:02}")
        })
        .unwrap_or_else(|_| "        ".to_string());
    let khz = e.peak_hz / 1000.0;
    let dur = if e.duration_ms >= 1000 {
        format!("{:.1}s", e.duration_ms as f32 / 1000.0)
    } else {
        format!("{}ms", e.duration_ms)
    };
    format!("{}  {:.2} kHz  {:.0} dB  {:>6}", time, khz, e.peak_db, dur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_since_absolute_iso8601() {
        let s = "2026-05-02T13:00:00Z";
        let result = parse_since(s);
        assert!(result.is_some());
        let dt = result.unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.hour(), 13);
    }

    #[test]
    fn parse_since_relative_seconds() {
        let now = OffsetDateTime::now_utc();
        let r = parse_since("30s").unwrap();
        let diff = (now - r).whole_seconds();
        assert!((29..=31).contains(&diff));
    }

    #[test]
    fn parse_since_relative_minutes() {
        let now = OffsetDateTime::now_utc();
        let r = parse_since("10m").unwrap();
        let diff = (now - r).whole_seconds();
        assert!((599..=601).contains(&diff));
    }

    #[test]
    fn parse_since_relative_hours() {
        let now = OffsetDateTime::now_utc();
        let r = parse_since("2h").unwrap();
        let diff = (now - r).whole_seconds();
        assert!((7199..=7201).contains(&diff));
    }

    #[test]
    fn parse_since_relative_days() {
        let now = OffsetDateTime::now_utc();
        let r = parse_since("1d").unwrap();
        let diff = (now - r).whole_seconds();
        assert!((86_399..=86_401).contains(&diff));
    }

    #[test]
    fn parse_since_invalid() {
        assert!(parse_since("garbage").is_none());
        assert!(parse_since("10x").is_none());
        assert!(parse_since("").is_none());
        assert!(parse_since("s").is_none());
    }

    #[test]
    fn format_event_short_duration() {
        let e = Event {
            ts: "2026-05-02T13:42:11Z".into(),
            peak_hz: 19120.5,
            peak_db: -32.1,
            duration_ms: 850,
        };
        let s = format_event(&e);
        assert!(s.contains("13:42:11"));
        assert!(s.contains("19.12 kHz"));
        assert!(s.contains("-32 dB"));
        assert!(s.contains("850ms"));
    }

    #[test]
    fn format_event_long_duration() {
        let e = Event {
            ts: "2026-05-02T15:08:33Z".into(),
            peak_hz: 20300.0,
            peak_db: -28.0,
            duration_ms: 1200,
        };
        let s = format_event(&e);
        assert!(s.contains("15:08:33"));
        assert!(s.contains("20.30 kHz"));
        assert!(s.contains("1.2s"));
    }
}
