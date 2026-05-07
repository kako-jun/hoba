# hoba

A minimal random number library with a small, focused API.

`hoba` provides a handful of straightforward functions backed by the operating
system's cryptographic random source. It aims to be a lightweight alternative
when you do not need the full surface of `rand`.

<!-- Demo recording goes here. -->
<!-- ![hoba demo](docs/demo.gif) -->

## Why infrasound is the default

The release default of `hoba`'s environmental quality monitor watches **a
single 1–10 Hz infrasound bucket that flips on frequency presence** — i.e.
the moment any tone in 1–10 Hz beats the ambient noise floor by at least
6 dB SNR, the detector trips. While the trigger is active, the **least
significant bit of every `random_u64()` is cleared** (mask
`0xFFFF_FFFF_FFFF_FFFE`, output forced even). Three design choices matter,
and they all come from either *Patlabor* or the original `hoba` design:

- **One bucket. Binary trigger. No graded depth.** In *Patlabor: The
  Movie* (1989), the HOS / バビロンプロジェクト trigger is binary: it
  fires or it doesn't. `hoba` 0.5.0 returns to that exactly — a single
  bucket spans 1.0–10.0 Hz, and any tone in that window flips the
  detector. The 1–4 bit graded depth that lived in 0.2.0–0.4.x is gone;
  the mask is fixed at LSB 1-bit, matching the original design in
  `notes/dev/hoba.md`.
- **Frequency presence, not amplitude.** The v0.3.x detector tested an
  absolute raw-power threshold, which silently re-calibrates with mic gain,
  room tone, and per-host noise floor. v0.4.0+ uses a **6 dB SNR check
  against a per-poll noise-floor estimate** (median bin power inside the
  trigger band, excluding the bucket's own window). The question the
  detector answers is "is the frequency present?", not "is the amplitude
  over my hand-tuned constant?" — closer to what the HOS lore actually
  requires.
- **Deterministic bias.** A binary trigger plus a fixed LSB-1 mask gives
  every triggered draw the same shape: output is even. Hidden features
  built on top (gacha biased toward even index, A/B test biased toward
  arm A, …) can be designed without worrying that depth 1 and depth 4
  paint different bits.

1–10 Hz is physically below what any consumer playback chain reproduces. A
laptop will not trigger this default by accident; a phone will not; a
YouTube video will not. The only sources that reliably reach this band are
real environmental events — **earthquakes, typhoon gusts, large industrial
machinery, big HVAC plants, subway trains rolling past**. That "doesn't fire
in everyday life" is the whole point. The default is supposed to fire on the
day you have already forgotten you ever depended on `hoba`.

If you actually want a band you can demo through speakers, you have two
options without recompiling: the `audible-test` cargo feature (single
bucket at 1.75 kHz, 1.0–2.5 kHz window), or `HOBA_BUCKETS` to point the
detector anywhere your hardware can reach. See
[Configuration](#configuration).

## Quickstart

```toml
[dependencies]
hoba = "0.5"
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
those conditions appear to be affecting entropy quality, the **LSB** of
every `random_u64` result is cleared (mask `0xFFFF_FFFF_FFFF_FFFE`, output
forced even). One function surfaces the state:

```rust
hoba::is_compromised();      // bool — false in normal conditions
```

Security-sensitive callers should check `is_compromised()` and either
skip RNG calls or fall back to another source.

To opt out of the monitor entirely:

```toml
[dependencies]
hoba = { version = "0.5", default-features = false }
```

## Configuration

The detector ships with two presets and accepts arbitrary runtime overrides
— same binary, no recompile.

### Library API

```rust
use hoba::audio::{Detector, DetectorConfig, MicSource};

// Production default: single 1–10 Hz infrasound bucket, 6 dB SNR
// threshold. Binary trigger, no graded depth. Will not fire on consumer
// audio — by design.
let cfg = DetectorConfig::release_default();

// Audible-band preset, reachable from a release build (no `audible-test`
// feature needed). Single bucket centred at 1.75 kHz, 1.0–2.5 kHz window
// — structurally symmetric to release_default. Use this for development,
// CI, and live demos.
let cfg = DetectorConfig::audible_test();

// Custom: sub-bass HVAC monitor, 20–50 Hz buckets, stricter SNR.
// Reachable through a bass amp / subwoofer. Any of the four centres
// firing is enough to flip the detector.
let cfg = DetectorConfig {
    buckets: vec![20.0, 30.0, 40.0, 50.0],
    snr_threshold_db: 10.0,
    peak_band_hz: (10.0, 60.0),
    sample_rate: 48_000,
    fft_size: 8192,
    bucket_half_width_hz: 5.0,
};

let detector = Detector::with_config(MicSource::new(), cfg);
```

### Environment variables

When `HOBA_MONITOR=1` auto-spawns the detector thread, three optional vars
override the compile-time default:

| Var              | Format                                    | Example                            |
| ---------------- | ----------------------------------------- | ---------------------------------- |
| `HOBA_BUCKETS`   | `<center_hz>,…` (comma-separated Hz)      | `HOBA_BUCKETS=1,3,5,10`            |
| `HOBA_SNR`       | non-negative dB SNR threshold (default 6) | `HOBA_SNR=10`                      |
| `HOBA_PEAK_BAND` | `<lo_hz>:<hi_hz>` (peak / floor band)     | `HOBA_PEAK_BAND=10:60`             |

Parse failures fall back silently to the default. Set `HOBA_DEBUG=1` to
surface them on stderr.

> **Removed in v0.5.0:** the legacy `HOBA_BUCKETS=hz:depth,…` form is still
> parsed for back-compat, but the `:depth` portion is ignored. The graded
> depth concept was retired with v0.5.0; under `HOBA_DEBUG=1` a one-line
> warning is printed.
>
> **Deprecated since v0.4.0:** `HOBA_THRESHOLD` was the v0.3.x raw
> band-power knob. Its unit (post-FFT power) does not translate to the dB
> SNR threshold, so its value is ignored and a one-line deprecation warning
> is printed under `HOBA_DEBUG=1`. Use `HOBA_SNR` instead.

### Picking a band

| Use case                                                 | Suggested config                                            |
| -------------------------------------------------------- | ----------------------------------------------------------- |
| Infrasound 1–10 Hz (default — earthquake, HVAC, gusts)   | leave unset — release default                               |
| Multi-point infrasound 1 / 3 / 5 / 10 Hz                 | `HOBA_BUCKETS=1,3,5,10`                                     |
| Sub-bass 20–50 Hz (bass amp, subway, big HVAC)           | `HOBA_BUCKETS=20,30,40,50` `HOBA_SNR=10`                    |
| Audible-test 1.75 kHz (CI / live demos)                  | `HOBA_BUCKETS=1750`                                         |

The `audible-test` cargo feature still exists as a convenience preset that
flips the compile-time default to 1–2.5 kHz. It is no longer the only path
to non-infrasound operation — env vars do the same thing on a release binary.

## Demo

A live demo is included:

```bash
# Audible-band demo — the recommended way to see it react to a tone.
cargo run --example babel --features audible-test

# Or against the infrasound release default (will sit quietly until
# something seismic actually happens):
cargo run --example babel
```

It prints scripture line by line; while the detector is compromised, the feed
collapses into a flood of BABEL.

### Trying the BABEL demo across two terminals

The simplest end-to-end self-loopback uses the `audible-test` feature, which
moves the trigger band from infrasound to 1.75 kHz so consumer speakers can
actually reproduce it.

```bash
# Terminal A — listen for the trigger
cargo run --example babel --features audible-test --release -- --listen --snr 30

# Terminal B — emit 1.75 kHz from the same Mac's default output
cargo run --example babel --features audible-test --release
```

Expected: Terminal B prints scripture and immediately collapses into a red
BABEL flood (it hears its own emission). Terminal A prints scripture quietly
until B starts; when B's tone reaches A's mic, A's screen also collapses into
BABEL. Stop B (Ctrl+C) and A returns to scripture within a second or two.

**Why `--snr 30`?** The default `audible_test()` preset uses 18 dB SNR (raised
in v0.5.1 from the original 6). Even 18 may be too lenient if your room has
loud HVAC, fans, or speakers playing nearby — `--snr 30` is a safer demo
value for typical laptop environments. The release-default infrasound preset
still uses 6 dB SNR because the 1–10 Hz band is essentially noiseless in
everyday rooms; the trigger is binary because nothing else lives there.

## Self-test your device

Microphone and speaker frequency response varies per device. Before
relying on the trigger, check whether your mic actually picks up the
target band loudly enough — i.e. with enough SNR over the local noise
floor for the production detector to fire:

```bash
hoba check                            # default 19/19.5/20/20.5 kHz, 5 s each
hoba check --list-devices             # enumerate cpal inputs/outputs
hoba check --bands 100,200,300        # arbitrary bands, no recompile
hoba check --bands 19000:1,19500:2    # legacy form: ':depth' silently dropped
hoba check --snr 10                   # tighten the SNR threshold to 10 dB
hoba check --threshold 10             # deprecated alias for --snr (still dB)
```

`hoba check` plays a sine on each band, measures the median peak and the
median noise floor at the mic, and prints a per-band table:

```text
band  target_hz  detected_hz  peak_db    noise_db    snr_db   verdict
   1     150.0      149.8     -44.9 dB   -88.4 dB   43.5 dB  PASS
```

A band PASSes when `snr_db ≥ snr_threshold_db` (default 6 dB, same as
`DetectorConfig::snr_threshold_db`). Exit code is `0` only when every band
passes.

The `check` subcommand defaults to the historical 19–21 kHz sweep because
that's a band consumer speakers actually try to reproduce, even badly. The
release-default infrasound band cannot meaningfully be self-tested with
ordinary hardware — that is the design.

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

## Migrating from v0.4.x to v0.5

v0.5.0 removed the graded depth concept. Three breaking changes:

| v0.4.x                                     | v0.5.0                                  |
| ------------------------------------------ | --------------------------------------- |
| `buckets: Vec<(f32, u8)>`                  | `buckets: Vec<f32>`                     |
| `Detector::compromised_depth() -> u8`      | `Detector::is_compromised() -> bool`    |
| `hoba::compromised_depth() -> u8`          | `hoba::is_compromised() -> bool`        |
| `HOBA_BUCKETS=hz:depth,…`                  | `HOBA_BUCKETS=hz,hz,hz`                 |
| Mask: 1–4 LSBs cleared (depth-graded)      | Mask: LSB only (`0xFFFF…FFFE`, even)    |

Backwards compatibility: `HOBA_BUCKETS=hz:depth,…` and `hoba check --bands
hz:depth,…` are still accepted — the `:depth` portion is silently dropped
and, when `HOBA_DEBUG=1`, a one-line warning notes that the depth concept
was removed in v0.5.0. Source-level callers that passed `(f32, u8)` tuples
in struct literals or that read `compromised_depth()` must update.

The motivation for the removal is on Issue #40: the v0.4.x graded depth
inherited from PR #5 contradicted the binary HOS lore in *Patlabor: The
Movie* (1989), and the residual `(f32, u8)` shape kept it alive in
`audible_test()` and on the public API even after v0.4.0 had collapsed
the production default to a single bucket. v0.5.0 finishes the job and
returns the library to the original `notes/dev/hoba.md` design.

## Documentation

Full API on [docs.rs/hoba](https://docs.rs/hoba).

## Inspired by

The name and the spirit are taken from *Hoba Eiichi* (帆場暎一), the
fictional programmer in *Patlabor: The Movie* (1989) whose code only
revealed its true behaviour under the right conditions. The release
default's infrasound band is a direct nod to HOS / バビロンプロジェクト —
something that does not fire in everyday life, and is not supposed to.

## License

MIT © kako-jun
