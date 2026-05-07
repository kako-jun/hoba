# Changelog

## v0.4.0 — Infrasound default, SNR-based detection

- **Release default switched to a single 1–10 Hz infrasound bucket that fires
  on frequency presence (SNR ≥ 6 dB) rather than absolute amplitude** —
  matching *Patlabor 1*'s HOS lore where the trigger is binary, not graded.
  One bucket centred on 5.5 Hz with `bucket_half_width_hz = 4.5` covers the
  entire 1–10 Hz window and pushes the mask depth straight to 4 anywhere
  inside it. Replaces both the previous v0.3.x 19–21 kHz ultrasonic
  placeholder *and* the early-v0.4 graded 1 / 3 / 5 / 10 Hz iteration.
- **Detection criterion moved from raw power to dB SNR.** `DetectorConfig`
  now exposes `snr_threshold_db` instead of `power_threshold`; the bucket
  fires when its peak beats a per-poll noise-floor estimate (median bin
  power across `peak_band_hz`, excluding each bucket's own window) by at
  least the threshold. Default 6 dB ≈ 2× signal-over-noise. The hand-
  calibrated absolute threshold from v0.3.x is gone — it silently re-tuned
  with mic gain and room tone, which is the opposite of what "monitor for
  trigger frequencies" should mean.
  - **Breaking (pre-1.0):** struct-literal `DetectorConfig` callers must
    rename `power_threshold` to `snr_threshold_db` and update the unit.
  - `Detector` gains `noise_floor_db()` and `snr_db()` accessors mirroring
    `peak_db()`, so live diagnostics can show the same three numbers the
    detector itself uses.
- **Env-var contract.** `HOBA_THRESHOLD` is deprecated; use `HOBA_SNR=<dB>`.
  v0.4.0 still detects `HOBA_THRESHOLD` so it counts as a "set an override"
  signal for back-compat tooling, but the value itself is ignored — the
  unit changed and silently reinterpreting the number would surprise
  callers. `HOBA_DEBUG=1` prints a one-line deprecation note.
- `DetectorConfig` also gains `fft_size` (65536 for the infrasound default
  so the 1 Hz end of the band is resolvable; 2048 elsewhere) and
  `bucket_half_width_hz` (4.5 Hz for the infrasound default so the single
  bucket spans the full trigger window). Both presets populate sensible
  values.
- `hoba check` output gains explicit `peak_db` / `noise_db` / `snr_db`
  columns; the per-band PASS condition is now `snr_db ≥ snr_threshold_db`.
  CLI flag `--threshold` is kept as a deprecated alias for `--snr` (both
  take a dB SNR value). Default sweep band remains 19–21 kHz — it is
  partially playable on consumer hardware; the infrasound release default
  cannot be meaningfully self-tested without seismic equipment.
- README: rewrote "Why infrasound is the default" around the binary-trigger
  + frequency-presence framing; band-picking guide updated for `HOBA_SNR`
  and the legacy graded-bucket recipe. Ultrasonic band-picking row removed.

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
