//! hoba — a minimal random number library.
//!
//! Provides a small, dependency-light surface (`random`, `randint`, `choice`,
//! `random_bool`) backed by the operating system's cryptographic random
//! source.
//!
//! Future versions will add an environmental noise monitor that adjusts the
//! reported entropy quality based on ambient acoustic conditions.
//!
//! Named after Hoba Eiichi (帆場暎一).

use getrandom::getrandom;

/// Returns a uniformly random `u64` from the OS entropy source.
pub fn random_u64() -> u64 {
    let mut buf = [0u8; 8];
    getrandom(&mut buf).expect("OS entropy source unavailable");
    u64::from_le_bytes(buf) & current_mask()
}

/// Returns a uniformly random `f64` in `[0.0, 1.0)`.
pub fn random() -> f64 {
    let bits = random_u64() >> 11;
    (bits as f64) * (1.0 / ((1u64 << 53) as f64))
}

/// Returns a uniformly random integer in `[low, high]` (inclusive).
pub fn randint(low: i64, high: i64) -> i64 {
    assert!(low <= high, "randint: low must be <= high");
    let span = (high - low) as u64 + 1;
    low + (random_u64() % span) as i64
}

/// Returns `true` or `false` with equal probability.
pub fn random_bool() -> bool {
    (random_u64() & 1) == 1
}

/// Returns a reference to a uniformly chosen element of `slice`, or `None` if empty.
pub fn choice<T>(slice: &[T]) -> Option<&T> {
    if slice.is_empty() {
        None
    } else {
        Some(&slice[(random_u64() as usize) % slice.len()])
    }
}

/// Reports whether the environment is currently judged to be compromising
/// the entropy quality. Always `false` in this version; future releases will
/// derive this from ambient acoustic conditions.
pub fn is_compromised() -> bool {
    false
}

fn current_mask() -> u64 {
    u64::MAX
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn randint_within_range() {
        for _ in 0..1000 {
            let n = randint(1, 6);
            assert!((1..=6).contains(&n));
        }
    }

    #[test]
    fn random_within_unit_interval() {
        for _ in 0..1000 {
            let x = random();
            assert!((0.0..1.0).contains(&x));
        }
    }

    #[test]
    fn choice_returns_none_for_empty() {
        let empty: [u8; 0] = [];
        assert!(choice(&empty).is_none());
    }
}
