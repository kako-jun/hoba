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
hoba check                       # default 19/19.5/20/20.5 kHz, 5 s each
hoba check --list-devices        # enumerate cpal inputs/outputs
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
