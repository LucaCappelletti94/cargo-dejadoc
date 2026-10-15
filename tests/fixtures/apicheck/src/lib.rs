//! Scanned by the scheduled API check, whose branch appends copies of these two doctests.

/// Helper with a doctest.
/// ```
/// fn helper() { }
/// ```
pub fn helper() {}

/// Unparseable body.
/// ```
/// @ nope nope
/// ```
pub fn unparsed() {}

/// A tilde fenced copy added by the API check.
/// ~~~
/// fn helper() { }
/// ~~~
pub fn helper_tilde() {}

/// An indented copy added by the API check.
///
///     @ nope nope
pub fn unparsed_indented() {}
