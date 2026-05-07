# Changelog

## v0.5.0 — Drop graded depth concept entirely

- **Removed `Vec<(f32, u8)>` bucket-with-depth API in favour of `Vec<f32>`** —
  Patlabor 1's HOS rampage is binary; the v0.4.x graded mask depth was
  inherited from PR #5 (added before the design returned to canon) and
  the residual `(f32, u8)` shape contradicted the v0.4.0 single-bucket
  default by silently keeping graded structure in `audible_test()`.
- **Mask is now hardcoded LSB 1-bit** (`0xFFFF_FFFF_FFFF_FFFE`,
  even-only) — restoring the original design in `notes/dev/hoba.md`
  ("世界の二択分岐が片側に倒れる" = single bit's worth of bias is enough
  to topple every binary decision).
- **`Detector::compromised_depth()` removed**; `is_compromised() -> bool`
  is the only state observation. Hidden features built on hoba can now
  be designed deterministically — every triggered draw biases by the
  same fixed transform, no surprise grading.
- `audible_test()` preset rewritten to single bucket centred at 1.75 kHz
  (1.0–2.5 kHz window via `bucket_half_width_hz = 750.0`), structurally
  symmetric to `release_default()`.
- `HOBA_BUCKETS` env var format simplified to `hz,hz,hz`. The legacy
  `hz:depth` form is parsed for back-compat but the depth portion is
  ignored with a `HOBA_DEBUG=1` warning.
- `hoba check --bands` accepts the same legacy `hz:depth` form for
  back-compat (depth dropped, optional `HOBA_DEBUG=1` warning); output
  table no longer shows a depth column.
- `examples/babel.rs` flood is now a single fixed-width line in uniform
  red — no per-line growth, no graded color escalation. `examples/demo.rs`
  reports `monitor=TRIGGER` / `--` instead of numeric mode labels.
- `Event` struct loses its `depth` field (binary trigger means the field
  carried no information); `hoba log` / `hoba watch` output drops the
  `depth N` column.
- Breaking (pre-1.0): callers using `compromised_depth()` or struct-
  literal `DetectorConfig` with `(f32, u8)` buckets must migrate. See
  the migration section in README.

## v0.4.0 — Infrasound default, SNR-based detection, runtime configuration

- **Release default switched to a single 1–10 Hz infrasound bucket that fires
  on frequency presence (SNR ≥ 6 dB) rather than absolute amplitude** —
  matching *Patlabor 1*'s HOS lore where the trigger is binary, not graded.
  One bucket centred on 5.5 Hz with `bucket_half_width_hz = 4.5` covers the
  entire 1–10 Hz window and pushes the mask depth straight to 4 anywhere
  inside it. Replaces the previous v0.3.x 19–21 kHz ultrasonic placeholder
  (PR #37, closes #36).
- **Detection criterion moved from raw power to dB SNR.** `DetectorConfig`
  now exposes `snr_threshold_db` instead of `power_threshold`; the bucket
  fires when its peak beats a per-poll noise-floor estimate (median bin
  power across `peak_band_hz`, excluding each bucket's own window) by at
  least the threshold. Default 6 dB ≈ 2× signal-over-noise. The hand-
  calibrated absolute threshold from v0.3.x is gone — it silently re-tuned
  with mic gain and room tone, which is the opposite of what "monitor for
  trigger frequencies" should mean (PR #37).
  - **Breaking (pre-1.0):** struct-literal `DetectorConfig` callers must
    rename `power_threshold` to `snr_threshold_db` and update the unit.
  - `Detector` gains `noise_floor_db()` and `snr_db()` accessors mirroring
    `peak_db()`, so live diagnostics can show the same three numbers the
    detector itself uses.
- **Env-var contract.** `HOBA_THRESHOLD` is deprecated; use `HOBA_SNR=<dB>`.
  v0.4.0 still detects `HOBA_THRESHOLD` so it counts as a "set an override"
  signal for back-compat tooling, but the value itself is ignored — the
  unit changed and silently reinterpreting the number would surprise
  callers. `HOBA_DEBUG=1` prints a one-line deprecation note (PR #37).
- **Runtime-configurable detection** (PR #35, closes #34): `DetectorConfig`
  exposes `buckets`, `peak_band_hz`, plus `HOBA_BUCKETS` /
  `HOBA_PEAK_BAND` env-var overrides on top of `HOBA_SNR`. Detector logic
  reads from the config; const-only paths removed. `DetectorConfig::from_env`
  builds a config straight off the environment for hidden-mode auto-spawn.
- **`hoba check` subcommand** (PR #33, closes #32): per-device
  frequency-response self-test with PASS/FAIL verdict, `--list-devices`,
  `--bands`, `--snr` (with `--threshold` retained as deprecated alias),
  `--listen-only`, `--input-device` / `--output-device`. Output gains
  explicit `peak_db` / `noise_db` / `snr_db` columns; default sweep band
  remains 19–21 kHz — it is partially playable on consumer hardware,
  whereas the infrasound release default cannot be meaningfully self-tested
  without seismic equipment.
- **`audible-test` cargo feature**: relocates the trigger band from 1–10 Hz
  infrasound up to 1–2.5 kHz so the detector can be exercised through
  ordinary speakers. CI / dev only — breaks the homage to the Famicom 2P
  mic / Hoba Eiichi.
- **`examples/babel.rs`** Patlabor-flavoured demo of the trigger flipping
  the output: now paints the BABEL flood red while in compromise (depth-
  graded yellow → red → bright red → bold bright red), suppresses the
  heartbeat diagnostic by default, and gates it behind `--diagnose` for
  opt-in instrumentation. ANSI escapes are skipped for non-tty stdout
  (PR #39, closes #38).
- `DetectorConfig` also gains `fft_size` (65536 for the infrasound default
  so the 1 Hz end of the band is resolvable; 2048 elsewhere) and
  `bucket_half_width_hz` (4.5 Hz for the infrasound default so the single
  bucket spans the full trigger window). Both presets populate sensible
  values.
- README: rewrote "Why infrasound is the default" around the binary-trigger
  + frequency-presence framing; band-picking guide updated for `HOBA_SNR`
  and the legacy graded-bucket recipe. Ultrasonic band-picking row removed.

## v0.3.0 — Event log hardening

- Atomic-rename event log writes with sibling lock file, retention pruning,
  and a process-global `dropped_event_count()` diagnostic surfacing silent
  write failures.
- `audio_active()` reports whether the background detector ever opened a
  real input stream — distinguishes "quiet environment" from "mic
  unreachable".
- `fs2` → `fs4` dependency migration (`fs2 0.4` is upstream-deprecated).
- All four shipped under PR #31.

## v0.2.0 — Detector, whitening, hoba CLI

- `MicSource` via `cpal` with silent-failure path (#3).
- FFT-based high-frequency detector with hold-off (#2).
- Frequency-graded mask depth 1–4 bits tied to 0.5 kHz buckets (#5).
- Auto-spawn mic monitor when `HOBA_MONITOR=1`; detector flag wired to
  `current_mask` (#4).
- `examples/demo.rs` interactive demo with opaque mode labels (#6, #14).
- Optional entropy-whitening pipeline: BLAKE3 + jitter + rdrand (#8).
- Persistent cross-process event log gated on `HOBA_MONITOR` (#12).
- `hoba` CLI binary with `log` / `watch` subcommands (#13).
- README polish: quickstart, environment monitor, demo, Famicom 2P-mic
  homage (#7).

## v0.1.x — Initial release

- Minimal RNG surface: `random`, `randint`, `choice`, `random_bool` backed
  by OS entropy.
- `AudioSource` trait + `SineSource` fixture for deterministic detector
  tests (#1).
- `mic` feature gates the background trigger monitor; without it `hoba` is
  an ordinary cryptographic-RNG wrapper.
