//! ```rust
//! let input = true;
//! let value = if input { true } else { false };
//! assert_eq!(value, true);
//! ```
//!
//! ```rust
//! let input = true;
//! let value = input;
//! assert_eq!(value, true);
//! ```
//!
//! ```rust
//! let input = false;
//! let value = if input { true } else { false };
//! assert_eq!(value, false);
//! ```

pub mod left {
    type Bit = bool;
    type Scalar = u32;

    pub fn first(p: Bit, q: Bit, a: Scalar, b: Scalar) -> (Scalar, Bit, Bit, bool, Bit, u8, u8) {
        let gate = !!p;
        let blocked = !(gate && q);
        let mask = 2u32 + 3u32;
        let left = a & mask;
        let right = b ^ 7u32;
        let combined = (left ^ right) ^ 9u32;
        let comparison = (a > b) == true;
        let copied = combined | combined;
        let choose = if p { true } else { false };
        let inverted = if !q { 1u8 } else { 2u8 };
        let literal = if true { 3u8 } else { 4u8 };
        (copied, gate, blocked, comparison, choose, inverted, literal)
    }

    pub fn same(x: Bit, y: Bit, c: Scalar, d: Scalar) -> (Scalar, Bit, Bit, bool, Bit, u8, u8) {
        let gate = x;
        let open = !gate || !y;
        let mask = 5u32;
        let second = 7u32 ^ d;
        let first = mask & c;
        let joined = 9u32 ^ (second ^ first);
        let order = d < c;
        let value = joined;
        let choose = x;
        let inverted = if y { 2u8 } else { 1u8 };
        let literal = 3u8;
        (value, gate, open, order, choose, inverted, literal)
    }

    pub fn different(p: Bit, q: Bit, a: Scalar, b: Scalar) -> (Scalar, Bit, Bit, bool, Bit, u8, u8) {
        let gate = !!p;
        let blocked = !(gate || q);
        let mask = 2u32 + 3u32;
        let left = a & mask;
        let right = b ^ 7u32;
        let combined = (left ^ right) ^ 9u32;
        let comparison = a < b;
        let copied = combined | combined;
        (copied, gate, blocked, comparison, p, 3u8, 4u8)
    }
}

pub mod right {
    pub fn first(p: bool, q: bool, a: u32, b: u32) -> (u32, bool, bool, bool, bool, u8, u8) {
        let gate = !!p;
        let blocked = !(gate && q);
        let mask = 2u32 + 3u32;
        let left = a & mask;
        let right = b ^ 7u32;
        let combined = (left ^ right) ^ 9u32;
        let comparison = (a > b) == true;
        let copied = combined | combined;
        let choose = if p { true } else { false };
        let inverted = if !q { 1u8 } else { 2u8 };
        let literal = if true { 3u8 } else { 4u8 };
        (copied, gate, blocked, comparison, choose, inverted, literal)
    }

    pub fn same(x: bool, y: bool, c: u32, d: u32) -> (u32, bool, bool, bool, bool, u8, u8) {
        let gate = x;
        let open = !gate || !y;
        let mask = 5u32;
        let second = 7u32 ^ d;
        let first = mask & c;
        let joined = 9u32 ^ (second ^ first);
        let order = d < c;
        let value = joined;
        let choose = x;
        let inverted = if y { 2u8 } else { 1u8 };
        let literal = 3u8;
        (value, gate, open, order, choose, inverted, literal)
    }

    pub fn different(p: bool, q: bool, a: u32, b: u32) -> (u32, bool, bool, bool, bool, u8, u8) {
        let gate = !!p;
        let blocked = !(gate || q);
        let mask = 2u32 + 3u32;
        let left = a & mask;
        let right = b ^ 7u32;
        let combined = (left ^ right) ^ 9u32;
        let comparison = a < b;
        let copied = combined | combined;
        (copied, gate, blocked, comparison, p, 3u8, 4u8)
    }
}

pub struct Owner;

impl Owner {
    pub fn first(p: bool, q: bool, a: u32, b: u32) -> (u32, bool, bool, bool, bool, u8, u8) {
        let gate = !!p;
        let blocked = !(gate && q);
        let mask = 2u32 + 3u32;
        let left = a & mask;
        let right = b ^ 7u32;
        let combined = (left ^ right) ^ 9u32;
        let comparison = (a > b) == true;
        let copied = combined | combined;
        let choose = if p { true } else { false };
        let inverted = if !q { 1u8 } else { 2u8 };
        let literal = if true { 3u8 } else { 4u8 };
        (copied, gate, blocked, comparison, choose, inverted, literal)
    }

    pub fn same(x: bool, y: bool, c: u32, d: u32) -> (u32, bool, bool, bool, bool, u8, u8) {
        let gate = x;
        let open = !gate || !y;
        let mask = 5u32;
        let second = 7u32 ^ d;
        let first = mask & c;
        let joined = 9u32 ^ (second ^ first);
        let order = d < c;
        let value = joined;
        let choose = x;
        let inverted = if y { 2u8 } else { 1u8 };
        let literal = 3u8;
        (value, gate, open, order, choose, inverted, literal)
    }

    pub fn different(p: bool, q: bool, a: u32, b: u32) -> (u32, bool, bool, bool, bool, u8, u8) {
        let gate = !!p;
        let blocked = !(gate || q);
        let mask = 2u32 + 3u32;
        let left = a & mask;
        let right = b ^ 7u32;
        let combined = (left ^ right) ^ 9u32;
        let comparison = a < b;
        let copied = combined | combined;
        (copied, gate, blocked, comparison, p, 3u8, 4u8)
    }
}
