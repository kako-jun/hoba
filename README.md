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
