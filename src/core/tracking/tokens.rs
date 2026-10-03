//! Shared token estimate for tracking, output guards, and savings tests.

/// Estimate token count from text using ~4 UTF-8 bytes = 1 token heuristic.
///
/// This is a fast approximation suitable for tracking purposes.
/// For precise counts, integrate with your LLM's tokenizer API.
///
/// # Formula
///
/// `tokens = ceil(bytes / 4)`
///
/// # Examples
///
/// ```
/// use rtk::tracking::estimate_tokens;
///
/// assert_eq!(estimate_tokens(""), 0);
/// assert_eq!(estimate_tokens("abcd"), 1);  // 4 bytes = 1 token
/// assert_eq!(estimate_tokens("abcde"), 2); // 5 bytes = ceil(1.25) = 2
/// assert_eq!(estimate_tokens("hello world"), 3); // 11 bytes = ceil(2.75) = 3
/// ```
pub fn estimate_tokens(text: &str) -> usize {
    estimate_tokens_from_len(text.len())
}

/// Token estimate from a raw byte length, for callers that hold a byte count rather
/// than a `&str` (e.g. non-UTF-8 captured output). Same ~4-bytes-per-token model as
/// [`estimate_tokens`].
pub fn estimate_tokens_from_len(len: usize) -> usize {
    (len as f64 / 4.0).ceil() as usize
}
