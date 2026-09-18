//! Scratch crate for proving a release end to end.

/// Adds one.
///
/// ```
/// let total = 1_000 + 1;
/// assert_eq!(total, 1001);
/// ```
pub fn bump(n: u32) -> u32 {
    n + 1
}

/// Adds one, documented twice by mistake.
///
/// ```
/// let total = 1000 + 1;
/// assert_eq!(total, 1001);
/// ```
pub fn bump_again(n: u32) -> u32 {
    n + 1
}
