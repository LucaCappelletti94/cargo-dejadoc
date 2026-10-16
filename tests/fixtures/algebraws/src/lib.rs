//! ```rust
//! let input = 11u32;
//! let value = input & input;
//! assert_eq!(value, 11u32);
//! ```
//!
//! ```rust
//! let renamed = 11u32;
//! let result = renamed;
//! assert_eq!(result, 11u32);
//! ```

pub mod left {
    type Scalar = u32;

    pub fn first(a: Scalar, b: Scalar) -> (Scalar, bool) {
        let mask = 2u32 + 3u32;
        let left = a & mask;
        let right = b ^ 7u32;
        let combined = (left ^ right) ^ 9u32;
        let comparison = a > b;
        let copied = combined | combined;
        (copied, comparison)
    }

    pub fn same(x: Scalar, y: Scalar) -> (Scalar, bool) {
        let mask = 5u32;
        let second = 7u32 ^ y;
        let first = mask & x;
        let joined = 9u32 ^ (second ^ first);
        let order = y < x;
        let value = joined;
        (value, order)
    }

    pub fn different(a: Scalar, b: Scalar) -> (Scalar, bool) {
        let mask = 2u32 + 3u32;
        let left = a & mask;
        let right = b ^ 7u32;
        let combined = (left ^ right) ^ 9u32;
        let comparison = a < b;
        let copied = combined | combined;
        (copied, comparison)
    }
}

pub mod right {
    pub fn first(a: u32, b: u32) -> (u32, bool) {
        let mask = 2u32 + 3u32;
        let left = a & mask;
        let right = b ^ 7u32;
        let combined = (left ^ right) ^ 9u32;
        let comparison = a > b;
        let copied = combined | combined;
        (copied, comparison)
    }

    pub fn same(x: u32, y: u32) -> (u32, bool) {
        let mask = 5u32;
        let second = 7u32 ^ y;
        let first = mask & x;
        let joined = 9u32 ^ (second ^ first);
        let order = y < x;
        let value = joined;
        (value, order)
    }

    pub fn different(a: u32, b: u32) -> (u32, bool) {
        let mask = 2u32 + 3u32;
        let left = a & mask;
        let right = b ^ 7u32;
        let combined = (left ^ right) ^ 9u32;
        let comparison = a > b;
        let copied = combined | combined;
        (copied, !comparison)
    }
}

pub struct Owner;

impl Owner {
    pub fn first(a: u32, b: u32) -> (u32, bool) {
        let mask = 2u32 + 3u32;
        let left = a & mask;
        let right = b ^ 7u32;
        let combined = (left ^ right) ^ 9u32;
        let comparison = a > b;
        let copied = combined | combined;
        (copied, comparison)
    }

    pub fn same(x: u32, y: u32) -> (u32, bool) {
        let mask = 5u32;
        let second = 7u32 ^ y;
        let first = mask & x;
        let joined = 9u32 ^ (second ^ first);
        let order = y < x;
        let value = joined;
        (value, order)
    }

    pub fn different(a: u32, b: u32) -> (u32, bool) {
        let mask = 2u32 + 3u32;
        let left = a & mask;
        let right = b ^ 7u32;
        let combined = (left ^ right) ^ 9u32;
        let comparison = a > b;
        let copied = combined | combined;
        (comparison as u32, copied != 0)
    }
}
