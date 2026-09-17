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

/// ```
/// fn big_shared() {
///     let a = 1;
///     let b = 2;
///     let c = 3;
///     let d = 4;
///     let e = 5;
///     let f = 6;
///     let g = 7;
///     let h = 8;
///     assert_eq!(a + b + c + d + e + f + g + h, 36);
/// }
/// ```
pub fn alpha_long_a() {}

/// ```
/// fn big_shared() {
///     let a = 1;
///     let b = 2;
///     let c = 3;
///     let d = 4;
///     let e = 5;
///     let f = 6;
///     let g = 7;
///     let h = 8;
///     assert_eq!(a + b + c + d + e + f + g + h, 36);
/// }
/// ```
pub fn alpha_long_b() {}

/// Intro
///
/// ```
/// fn shared() { }
/// ```

pub fn alpha_added_doc_end() {}
