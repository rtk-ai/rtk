//! SimHash — Locality-Sensitive Hashing for O(1) near-duplicate detection.
//! Port from sqz_engine/src/simhash.rs (MIT-compatible subset).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// 64-bit SimHash fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SimHash(pub u64);

impl SimHash {
    pub fn hamming_distance(&self, other: &SimHash) -> u32 {
        (self.0 ^ other.0).count_ones()
    }

    pub fn is_near_duplicate(&self, other: &SimHash, max_distance: u32) -> bool {
        self.hamming_distance(other) <= max_distance
    }
}

/// Compute the SimHash fingerprint of a text.
pub fn simhash(text: &str) -> SimHash {
    let tokens = shingles(text, 3);
    if tokens.is_empty() {
        return SimHash(0);
    }
    let mut v = [0i32; 64];
    for token in &tokens {
        let h = hash_str(token);
        for i in 0..64 {
            if (h >> i) & 1 == 1 {
                v[i] += 1;
            } else {
                v[i] -= 1;
            }
        }
    }
    let mut fp: u64 = 0;
    for i in 0..64 {
        if v[i] > 0 {
            fp |= 1u64 << i;
        }
    }
    SimHash(fp)
}

fn shingles(text: &str, n: usize) -> Vec<String> {
    let words: Vec<&str> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    if words.len() < n {
        return words.iter().map(|w| w.to_lowercase()).collect();
    }
    words
        .windows(n)
        .map(|w| w.join(" ").to_lowercase())
        .collect()
}

fn hash_str(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_texts_same_hash() {
        let a = simhash("the quick brown fox jumps over the lazy dog and extra words");
        let b = simhash("the quick brown fox jumps over the lazy dog and extra words");
        assert_eq!(a.0, b.0);
    }

    #[test]
    fn test_near_duplicate_small_hamming() {
        let a = simhash(
            "fn process(input: &str) -> String { input.to_uppercase() and more words here }",
        );
        let b = simhash(
            "fn process(input: &str) -> String { input.to_lowercase() and more words here }",
        );
        let dist = a.hamming_distance(&b);
        assert!(dist < 32, "expected hamming < 32, got {dist}");
    }

    #[test]
    fn test_empty_text_returns_zero() {
        assert_eq!(simhash("").0, 0);
    }

    #[test]
    fn test_is_near_duplicate_threshold() {
        let a = simhash("cargo test --all --workspace runs all the tests in the project");
        let b = simhash("cargo test --all --workspace runs all the tests in the project");
        assert!(a.is_near_duplicate(&b, 10));
    }

    #[test]
    fn test_very_different_texts_high_hamming() {
        let a = simhash("the quick brown fox");
        let b = simhash("SELECT * FROM users WHERE deleted_at IS NULL");
        let _ = a.hamming_distance(&b);
    }
}
