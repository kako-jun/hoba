# hoba

A minimal random number library with a small, focused API.

`hoba` provides a handful of straightforward functions backed by the operating
system's cryptographic random source. It aims to be a lightweight alternative
when you do not need the full surface of `rand`.

The name comes from *hoba* (朴葉), the magnolia leaf used to wrap food in
Hida cuisine — light, simple, and unobtrusive.

## Install

```toml
[dependencies]
hoba = "0.1"
```

## Usage

```rust
let r = hoba::random();          // f64 in [0.0, 1.0)
let n = hoba::randint(1, 6);     // i64 in [1, 6]
let b = hoba::random_bool();     // true or false
let pick = hoba::choice(&["a", "b", "c"]);
```

## Roadmap

A future release will add an optional environmental quality monitor that
reports on ambient acoustic conditions affecting entropy.

## License

MIT © kako-jun
