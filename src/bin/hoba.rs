//! `hoba` command-line tool. Reads the shared event log written by
//! hoba-using processes when `HOBA_MONITOR=1` is set.

use std::io::{self, Write};
use std::time::Duration;

use clap::{Parser, Subcommand};
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
        #[arg(long)]
        since: Option<String>,
        /// Output raw JSONL instead of human-readable lines.
        #[arg(long)]
        json: bool,
    },
    /// Live-tail: print new events as they are appended to the log.
    Watch,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Log { since, json } => cmd_log(since, json),
        Cmd::Watch => cmd_watch(),
    }
}

fn cmd_log(since: Option<String>, json: bool) {
    let now = OffsetDateTime::now_utc();
    let cutoff = since
        .as_deref()
        .and_then(parse_since)
        .unwrap_or_else(|| now - time::Duration::days(30));
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
    format!(
        "{}  {:.2} kHz  {:.0} dB  {:>6}  depth {}",
        time, khz, e.peak_db, dur, e.depth
    )
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
            depth: 1,
        };
        let s = format_event(&e);
        assert!(s.contains("13:42:11"));
        assert!(s.contains("19.12 kHz"));
        assert!(s.contains("-32 dB"));
        assert!(s.contains("850ms"));
        assert!(s.contains("depth 1"));
    }

    #[test]
    fn format_event_long_duration() {
        let e = Event {
            ts: "2026-05-02T15:08:33Z".into(),
            peak_hz: 20300.0,
            peak_db: -28.0,
            duration_ms: 1200,
            depth: 3,
        };
        let s = format_event(&e);
        assert!(s.contains("15:08:33"));
        assert!(s.contains("20.30 kHz"));
        assert!(s.contains("1.2s"));
        assert!(s.contains("depth 3"));
    }
}
