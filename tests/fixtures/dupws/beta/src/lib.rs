/// Beta copy of the shared doctest, with a hidden line kept in the body.
/// ```
/// # let _h = 1;
/// fn shared() { }
/// ```
pub fn beta_fn() {}

/// An unparseable body shared with alpha.
/// ```
/// @ nope nope
/// ```
pub fn beta_unparsed() {}

pub mod util;
