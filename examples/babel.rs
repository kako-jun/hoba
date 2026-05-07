//! Patlabor-flavored demo: streams scripture line by line; while the
//! detector is compromised, the feed collapses into a flood of BABEL.
//!
//! Run (recommended for live demos):
//!
//! ```text
//! cargo run --example babel --features audible-test
//! ```
//!
//! With `--features audible-test` the trigger sits at 1 kHz; the demo
//! emits that from the default output device, the mic loops it back, and
//! the detector flips. Pass `--listen` to disable emission and use an
//! external tone source instead.
//!
//! Without `--features audible-test`, the detector watches the **release
//! default** infrasound band — a single 1–10 Hz bucket that fires at depth
//! 4 anywhere in the window (no graded depth, matching the Patlabor HOS).
//! No consumer speaker can reproduce that, by design — see
//! `DetectorConfig::release_default` for the rationale. The example will
//! start, print scripture, and quietly wait. To actually trigger it you
//! need real environmental infrasound (earthquake, typhoon gust, large
//! HVAC, subway) or to override the band via `HOBA_BUCKETS` / the
//! `--bands` flag.
//!
//! A diagnostic line on stderr shows what the detector is observing:
//! `[depth=N peak=AAA Hz BB.B dB]`. If `peak_db` stays near silence
//! (-90 dB or lower) while `--emit` is on, the speaker → mic loopback is
//! broken (output muted, mic missing/permission denied, BlueTooth headset
//! splitting in/out, etc.).

use std::io::Write;
use std::time::{Duration, Instant};

use hoba::audio::{AudioSource, Detector, DetectorConfig, MicSource};

/// Compile-time hint for `--emit`: the lowest bucket center in the active
/// preset. Under `audible-test` the example can actually loop this back
/// through speakers; under the release default (infrasound) it is well
/// below what cpal will play meaningfully, and emission is forced off
/// unless the user explicitly supplies their own `--bands`.
#[cfg(feature = "audible-test")]
const TRIGGER_HZ: f32 = 1_000.0;
/// Centre of the single 1–10 Hz infrasound bucket under the release
/// default. Used as the `--emit` fallback frequency, but cpal cannot
/// meaningfully play it — the demo forces listen-only mode below.
#[cfg(not(feature = "audible-test"))]
const TRIGGER_HZ: f32 = 5.5;
/// True when the compile-time default trigger band is something a consumer
/// speaker can actually reproduce. Off for the infrasound release default.
#[cfg(feature = "audible-test")]
const DEFAULT_BAND_PLAYABLE: bool = true;
#[cfg(not(feature = "audible-test"))]
const DEFAULT_BAND_PLAYABLE: bool = false;

/// Parses `--bands "<hz>:<depth>,..."` into a `Vec<(f32, u8)>`. Each item must
/// carry an explicit depth (1..=4). Whitespace tolerated; empty list rejected.
fn parse_bands_arg(s: &str) -> Result<Vec<(f32, u8)>, String> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (hz_s, d_s) = trimmed
            .split_once(':')
            .ok_or_else(|| format!("'{trimmed}' missing ':<depth>'"))?;
        let hz: f32 = hz_s
            .trim()
            .parse()
            .map_err(|e| format!("center_hz '{hz_s}' invalid: {e}"))?;
        let d: u8 = d_s
            .trim()
            .parse()
            .map_err(|e| format!("depth '{d_s}' invalid: {e}"))?;
        if !(1..=4).contains(&d) {
            return Err(format!("depth must be 1..=4: {d}"));
        }
        if !hz.is_finite() || hz <= 0.0 {
            return Err(format!("center_hz must be positive and finite: {hz}"));
        }
        out.push((hz, d));
    }
    if out.is_empty() {
        return Err("--bands needs at least one '<hz>:<depth>' item".into());
    }
    Ok(out)
}

/// Reads `--bands <s>` / `--threshold <f>` from `args` if present.
fn parse_value_arg(args: &[String], key: &str) -> Option<String> {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == key {
            if let Some(v) = iter.next() {
                return Some(v.clone());
            }
        } else if let Some(rest) = a.strip_prefix(&format!("{key}=")) {
            return Some(rest.to_string());
        }
    }
    None
}

const VERSES: &[&str] = &[
    "In the beginning God created the heavens and the earth.",
    "And the earth was without form, and void; and darkness was upon the face of the deep.",
    "And God said, Let there be light: and there was light.",
    "And God saw the light, that it was good.",
    "In the beginning was the Word, and the Word was with God, and the Word was God.",
    "And the whole earth was of one language, and of one speech.",
    "And they said, Go to, let us build us a city and a tower, whose top may reach unto heaven.",
    "And the Lord came down to see the city and the tower.",
    "Behold, the people is one, and they have all one language.",
    "Go to, let us go down, and there confound their language,",
    "that they may not understand one another's speech.",
    "Therefore is the name of it called Babel.",
];

const SCRIPTURE_CADENCE: Duration = Duration::from_millis(1500);
const BABEL_CADENCE: Duration = Duration::from_millis(50);
const HEARTBEAT_CADENCE: Duration = Duration::from_millis(250);

fn babel_line(depth: u8) -> String {
    // depth 1: 16 words, depth 4: 40 words — terminal fills faster the deeper it goes.
    let count = 8 + (depth as usize) * 8;
    "BABEL ".repeat(count).trim_end().to_string()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list-devices") {
        list_devices();
        return;
    }
    // Emission policy: `--listen` / `--no-emit` always wins. Otherwise emit
    // when (a) the user explicitly supplied `--bands` (they know the band is
    // playable), or (b) the compile-time default band is itself playable
    // (audible-test feature). Under the infrasound release default with no
    // `--bands`, there is nothing meaningful to emit; the demo runs in
    // listen-only mode and waits for real environmental infrasound.
    let listen_flag = args.iter().any(|a| a == "--listen" || a == "--no-emit");
    let bands_flag_present = args
        .iter()
        .any(|a| a == "--bands" || a.starts_with("--bands="));
    let emit_enabled = !listen_flag && (bands_flag_present || DEFAULT_BAND_PLAYABLE);

    // Optional CLI overrides for the detector. Falling back to the compile-time
    // default keeps the existing Patlabor demo behaviour untouched when neither
    // flag is passed.
    let bands_override = match parse_value_arg(&args, "--bands") {
        Some(s) => match parse_bands_arg(&s) {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("--bands: {e}");
                std::process::exit(2);
            }
        },
        None => None,
    };
    let threshold_override = match parse_value_arg(&args, "--threshold") {
        Some(s) => match s.trim().parse::<f32>() {
            Ok(v) if v.is_finite() && v >= 0.0 => Some(v),
            _ => {
                eprintln!("--threshold: must be non-negative and finite, got {s:?}");
                std::process::exit(2);
            }
        },
        None => None,
    };

    if !DEFAULT_BAND_PLAYABLE && !bands_flag_present {
        eprintln!(
            "(release default: a single 1–10 Hz infrasound bucket, depth 4. \
No consumer speaker can play this; the demo will sit quietly until your \
office HVAC kicks in or a typhoon strolls past. Use --features audible-test \
for a 1 kHz loopback demo, or --bands <hz>:<depth>,... to point the \
detector somewhere your hardware can actually reach.)"
        );
    }

    let mic = MicSource::new();
    if mic.is_active() {
        eprintln!("(mic: active, sample_rate={} Hz)", mic.sample_rate());
    } else {
        eprintln!("(mic: INACTIVE — no input device or permission denied)");
    }

    // Emit the lowest configured bucket if the user supplied --bands; otherwise
    // stay with the compile-time TRIGGER_HZ so the demo keeps working as before.
    let emit_hz = bands_override
        .as_ref()
        .and_then(|v| {
            v.iter()
                .map(|(hz, _)| *hz)
                .fold(None, |acc: Option<f32>, hz| {
                    Some(acc.map_or(hz, |a| a.min(hz)))
                })
        })
        .unwrap_or(TRIGGER_HZ);

    let _stream_guard = if emit_enabled {
        match emitter::start(emit_hz) {
            Ok((g, dev)) => {
                eprintln!("(emit: {emit_hz:.0} Hz from \"{dev}\" — pass --listen to disable)");
                Some(g)
            }
            Err(e) => {
                eprintln!("(emit failed: {e}; falling back to listen-only)");
                None
            }
        }
    } else {
        eprintln!("(--listen mode: emission disabled; provide your own tone source)");
        None
    };
    eprintln!("(Ctrl+C to stop)\n");

    // Build the detector. If neither override was supplied, use the compile-time
    // default; otherwise start from the default and patch in the user's values.
    let mut detector = if bands_override.is_some() || threshold_override.is_some() {
        let mut cfg = DetectorConfig::default();
        if let Some(buckets) = bands_override {
            cfg.peak_band_hz = DetectorConfig::peak_band_from_buckets(&buckets);
            cfg.buckets = buckets;
        }
        if let Some(t) = threshold_override {
            cfg.power_threshold = t;
        }
        eprintln!(
            "(detector: buckets={:?} threshold={} peak_band={:?})",
            cfg.buckets, cfg.power_threshold, cfg.peak_band_hz
        );
        Detector::with_config(mic, cfg)
    } else {
        Detector::with_source(mic)
    };
    let mut idx = 0usize;
    let mut next_print = Instant::now();
    let mut next_heartbeat = Instant::now();

    loop {
        detector.poll();
        let depth = detector.depth();
        let now = Instant::now();

        if now >= next_heartbeat {
            eprintln!(
                "[depth={} peak={:>5.0} Hz {:>5.1} dB]",
                depth,
                detector.peak_hz(),
                detector.peak_db()
            );
            next_heartbeat = now + HEARTBEAT_CADENCE;
        }

        if now >= next_print {
            if depth > 0 {
                println!("{}", babel_line(depth));
                next_print = now + BABEL_CADENCE;
            } else {
                println!("{}", VERSES[idx % VERSES.len()]);
                idx += 1;
                next_print = now + SCRIPTURE_CADENCE;
            }
            std::io::stdout().flush().ok();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "mic")]
fn list_devices() {
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
    eprintln!("host: {}", host.id().name());
    eprintln!("default input : {default_input}");
    eprintln!("default output: {default_output}\n");

    eprintln!("--- inputs ---");
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
                eprintln!("[{i}] {name}  | {cfg}");
            }
        }
        Err(e) => eprintln!("(input enumeration failed: {e})"),
    }

    eprintln!("\n--- outputs ---");
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
                eprintln!("[{i}] {name}  | {cfg}");
            }
        }
        Err(e) => eprintln!("(output enumeration failed: {e})"),
    }
}

#[cfg(not(feature = "mic"))]
fn list_devices() {
    eprintln!("(mic feature is disabled — no devices to list)");
}

#[cfg(feature = "mic")]
mod emitter {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    pub struct Guard {
        _stream: cpal::Stream,
    }

    pub fn start(freq_hz: f32) -> Result<(Guard, String), String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no default output device".to_string())?;
        let device_name = device.name().unwrap_or_else(|_| "?".to_string());
        let config = device
            .default_output_config()
            .map_err(|e| format!("default_output_config: {e}"))?;
        let sample_rate = config.sample_rate().0 as f32;
        let channels = config.channels() as usize;
        let mut phase = 0.0f32;
        let step = std::f32::consts::TAU * freq_hz / sample_rate;
        let amplitude = 0.5f32;

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device
                .build_output_stream(
                    &config.into(),
                    move |buf: &mut [f32], _| {
                        for frame in buf.chunks_mut(channels) {
                            let s = amplitude * phase.sin();
                            phase += step;
                            if phase > std::f32::consts::TAU {
                                phase -= std::f32::consts::TAU;
                            }
                            for slot in frame.iter_mut() {
                                *slot = s;
                            }
                        }
                    },
                    |err| eprintln!("output stream error: {err}"),
                    None,
                )
                .map_err(|e| format!("build_output_stream: {e}"))?,
            other => return Err(format!("unsupported output sample format: {other:?}")),
        };
        stream.play().map_err(|e| format!("stream.play: {e}"))?;
        Ok((Guard { _stream: stream }, device_name))
    }
}

#[cfg(not(feature = "mic"))]
mod emitter {
    pub struct Guard;
    pub fn start(_freq_hz: f32) -> Result<(Guard, String), String> {
        Err("mic feature is disabled".to_string())
    }
}
