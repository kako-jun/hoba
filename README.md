# hoba

A minimal random number library with a small, focused API.

`hoba` provides a handful of straightforward functions backed by the operating
system's cryptographic random source. It aims to be a lightweight alternative
when you do not need the full surface of `rand`.

<!-- Demo recording goes here. -->
<!-- ![hoba demo](docs/demo.gif) -->

## Quickstart

```toml
[dependencies]
hoba = "0.1"
```

```rust
let r = hoba::random();                          // f64 in [0.0, 1.0)
let n = hoba::randint(1, 6);                     // i64 in [1, 6]
let b = hoba::random_bool();                     // true or false
let pick = hoba::choice(&["a", "b", "c"]);       // Option<&&str>
```

That is the whole user-facing surface for normal use.

## Environment quality monitor

When the `mic` feature is enabled (default), `hoba` runs a lightweight
background monitor that observes ambient acoustic conditions. While
those conditions appear to be affecting entropy quality, the low bits
of every `random_u64` result are masked. Two functions surface the
state:

```rust
hoba::is_compromised();      // bool — false in normal conditions
hoba::compromised_depth();   // u8   — 0 in normal conditions
```

Security-sensitive callers should check `is_compromised()` and either
skip RNG calls or fall back to another source.

To opt out of the monitor entirely:

```toml
[dependencies]
hoba = { version = "0.1", default-features = false }
```

## Configuration

The detector ships with two presets and accepts arbitrary runtime overrides
— same binary, no recompile.

### Library API

```rust
use hoba::audio::{Detector, DetectorConfig, MicSource};

// Production default (19–21 kHz, threshold 10_000) — equivalent to Detector::new().
let cfg = DetectorConfig::release_default();

// Audible-band preset, reachable from a release build (no `audible-test` feature needed).
let cfg = DetectorConfig::audible_test();

// Custom: sub-bass HVAC monitor, 20–50 Hz, lower power threshold.
let cfg = DetectorConfig {
    buckets: vec![(20.0, 1), (30.0, 2), (40.0, 3), (50.0, 4)],
    power_threshold: 1_000.0,
    peak_band_hz: (10.0, 60.0),
    sample_rate: 48_000,
};

let detector = Detector::with_config(MicSource::new(), cfg);
```

### Environment variables

When `HOBA_MONITOR=1` auto-spawns the detector thread, three optional vars
override the compile-time default:

| Var               | Format                                 | Example                       |
| ----------------- | -------------------------------------- | ----------------------------- |
| `HOBA_BUCKETS`    | `<center_hz>:<depth>,…` (depth 1..=4)  | `HOBA_BUCKETS=20:1,30:2,40:3,50:4` |
| `HOBA_THRESHOLD`  | non-negative number (raw band power)   | `HOBA_THRESHOLD=1000`         |
| `HOBA_PEAK_BAND`  | `<lo_hz>:<hi_hz>` (peak reporting band)| `HOBA_PEAK_BAND=10:60`        |

Parse failures fall back silently to the default. Set `HOBA_DEBUG=1` to surface
them on stderr.

### Picking a band

| Use case                                    | Suggested config                                     |
| ------------------------------------------- | ---------------------------------------------------- |
| Sub-bass 20–50 Hz (bass amp, HVAC, subway)  | `HOBA_BUCKETS=20:1,30:2,40:3,50:4`                   |
| Ultrasonic 19–21 kHz (default, piezo, mice) | leave unset — release default                        |
| Audible-test 1–2.5 kHz (CI / live demos)    | `HOBA_BUCKETS=1000:1,1500:2,2000:3,2500:4` `HOBA_THRESHOLD=100` |

The `audible-test` cargo feature still exists as a convenience preset that
flips the compile-time default. It is no longer the only path to non-ultrasonic
operation — env vars do the same thing on a release binary.

## Demo

A live demo is included:

```bash
cargo run --example demo
```

It prints a `randint(1, 6)` once per second alongside the current
detector state. Try the demo while playing different ambient sounds.

## Self-test your device

Microphone and speaker frequency response varies per device. Before
relying on the trigger, check whether your mic actually picks up the
target band loudly enough:

```bash
hoba check                            # default 19/19.5/20/20.5 kHz, 5 s each
hoba check --list-devices             # enumerate cpal inputs/outputs
hoba check --bands 100,200,300        # arbitrary bands, no recompile
hoba check --bands 19000:1,19500:2    # explicit (hz:depth) pairs
hoba check --threshold 1000           # override the raw-power threshold
```

`hoba check` plays a sine on each band, measures the median peak at the
mic, and prints a per-band PASS/FAIL plus an overall verdict. Exit code
is `0` only when every band passes.

Three common scenarios:

1. Built-in speaker and mic with the `audible-test` feature:

   ```bash
   cargo run --bin hoba --features cli,audible-test -- \
       check --bands 1000 --duration 5
   ```

2. High-grade USB mic with a bass amp providing the tone — measure
   30–100 Hz response without a tweeter:

   ```bash
   hoba check --listen-only --bands 60,80,100 \
       --input-device "<USB mic name from --list-devices>"
   ```

3. iPhone tone generator emitting 19 kHz, Mac mic measuring:

   ```bash
   hoba check --listen-only
   ```

## Documentation

Full API on [docs.rs/hoba](https://docs.rs/hoba).

## Inspired by

The name and the spirit are taken from *Hoba Eiichi* (帆場暎一), in tribute
to a fictional programmer whose code only revealed its true behavior under
the right conditions. The environment monitor itself is a small homage to
the Famicom 2P controller microphone — a hidden input channel a few games
quietly used as a secret.

## License

MIT © kako-jun
