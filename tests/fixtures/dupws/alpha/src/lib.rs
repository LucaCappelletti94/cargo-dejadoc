//! File-level doc for alpha.
//!
//! ```
//! let file_level = true;
//! ```

// dejadoc demo touch, review comments render on the sites in this diff.
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
/// fn shared() { }
/// ```
pub fn alpha_added() {}
