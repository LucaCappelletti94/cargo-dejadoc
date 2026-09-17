/// Beta copy of the shared doctest, with a hidden line kept in the body.
/// ```
/// # let _h = 1;
/// fn shared() { }
/// ```
pub fn beta_fn() {}

/// Unparseable body shared with alpha.
/// ```
/// @ nope nope
/// ```
pub fn beta_unparsed() {}

pub mod util;

/// ```
/// let mut values = vec![3, 1, 2];
/// values.sort();
/// assert_eq!(values, vec![1, 2, 3]);
/// ```
pub fn sort_values() {}
