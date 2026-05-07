# Changelog

## v0.4.0 — Infrasound default

- **Release default switched to infrasound 1 / 3 / 5 / 10 Hz**, replacing the
  previous 19–21 kHz ultrasonic placeholder. Aligns the default with the
  *Patlabor* HOS homage (low-frequency wind resonance) and intentionally
  removes the ability to fire from consumer speakers. Override with
  `HOBA_BUCKETS` or `Detector::with_config` if you need a different band.
- `DetectorConfig` gains two fields: `fft_size` (so the infrasound default can
  use a 65536-point window for sub-Hz resolution while the audible-test preset
  stays at 2048) and `bucket_half_width_hz` (so closely-spaced infrasound
  buckets do not pollute each other). `release_default()` and
  `audible_test()` populate sensible values; struct-literal callers must add
  the new fields.
- `hoba check` self-test still defaults to 19–21 kHz — that band is at least
  partially playable on consumer hardware; the new infrasound default cannot
  be meaningfully self-tested without seismic equipment.
- README: front-loaded a "Why infrasound is the default" section and removed
  ultrasonic-as-default messaging from the band-picking guide. Restoring the
  historical ultrasonic band is documented as a one-line env-var override.

## v0.3.x — Runtime-configurable buckets, hardening, and `hoba check`

- v0.3.1 (PR #35): `DetectorConfig` exposes `buckets`, `power_threshold`,
  `peak_band_hz`, plus `HOBA_BUCKETS` / `HOBA_THRESHOLD` / `HOBA_PEAK_BAND`
  env-var overrides. Detector logic reads from the config; const-only paths
  removed.
- v0.3.0 (PR #31): on-disk event log gains atomic-rename writes, sibling lock
  file, retention pruning, and a process-global `dropped_event_count()`
  diagnostic. `audio_active()` reports whether the background detector ever
  opened a real input stream.
- `hoba check` subcommand (PR #33): per-device frequency-response self-test
  with PASS/FAIL verdict, `--list-devices`, `--bands`, `--threshold`,
  `--listen-only`, `--input-device` / `--output-device`.

## v0.2.0

- `audible-test` cargo feature: relocates the trigger band to 1–2.5 kHz so
  the detector can be exercised through ordinary speakers.
- BABEL example: Patlabor-flavoured demo of the trigger flipping the output.

## v0.1.x

- Initial release: minimal RNG surface (`random`, `randint`, `choice`,
  `random_bool`) backed by OS entropy; `mic` feature adds the background
  trigger monitor with bucket-mask depth on `random_u64`.
