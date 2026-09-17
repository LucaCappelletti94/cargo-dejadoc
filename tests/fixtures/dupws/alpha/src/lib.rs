//! File-level doc for alpha.
//!
//! ```
//! let file_level = true;
//! ```

pub mod util;

/// Alpha function.
/// ```
/// fn shared() { }
/// ```
pub fn alpha_fn() {}

/// Formatting variant of the same doctest.
/// ```
///
///     fn shared() { }   // trailing comment
///
/// ```
pub fn alpha_variant() {}

/// Allowed duplicate; carries the dejadoc token.
/// ```rust,dejadoc
/// fn shared() { }
/// ```
pub fn alpha_allowed() {}

/// Unparseable body shared with beta.
/// ```
/// @ nope nope
/// ```
pub fn alpha_unparsed() {}

/// ```
/// let mut items = vec![3, 1, 2];
/// items.sort();
/// assert_eq!(items, vec![1, 2, 3]);
/// ```
pub fn sort_items() {}
