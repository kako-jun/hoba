//! Patlabor-flavored demo: streams scripture line by line; while the
//! detector is compromised, the feed collapses into a flood of BABEL.
//!
//! Run:
//!
//! ```text
//! cargo run --example babel --features audible-test
//! ```
//!
//! By default the demo emits the trigger tone from the default output
//! device, the mic loops it back, and the detector flips. Pass `--listen`
//! to disable emission and use an external tone source instead.
//!
//! A diagnostic line on stderr shows what the detector is observing:
//! `[depth=N peak=AAA Hz BB.B dB]`. If `peak_db` stays near silence
//! (-90 dB or lower) while `--emit` is on, the speaker → mic loopback is
//! broken (output muted, mic missing/permission denied, BlueTooth headset
//! splitting in/out, etc.).
//!
//! Without `--features audible-test` the trigger sits at 19 kHz, which
//! most consumer speakers cannot reproduce — use the audible band for
//! live demos.

use std::io::Write;
use std::time::{Duration, Instant};

use hoba::audio::{AudioSource, Detector, MicSource};

#[cfg(feature = "audible-test")]
const TRIGGER_HZ: f32 = 1_000.0;
#[cfg(not(feature = "audible-test"))]
const TRIGGER_HZ: f32 = 19_000.0;

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
    let emit_enabled = !args.iter().any(|a| a == "--listen" || a == "--no-emit");

    let mic = MicSource::new();
    if mic.is_active() {
        eprintln!("(mic: active, sample_rate={} Hz)", mic.sample_rate());
    } else {
        eprintln!("(mic: INACTIVE — no input device or permission denied)");
    }

    let _stream_guard = if emit_enabled {
        match emitter::start(TRIGGER_HZ) {
            Ok((g, dev)) => {
                eprintln!("(emit: {TRIGGER_HZ:.0} Hz from \"{dev}\" — pass --listen to disable)");
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

    let mut detector = Detector::with_source(mic);
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
