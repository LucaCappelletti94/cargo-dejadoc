//! ```rust
//! let input = 7u32;
//! let mask = input & 15u32;
//! let copied = mask;
//! assert_eq!(copied ^ 3u32, 4u32);
//! ```
//!
//! ```rust
//! let input = 7u32;
//! let mask = input & 15u32;
//! assert_eq!(mask ^ 3u32, 4u32);
//! ```
//!
//! ```rust
//! let input = 7u32;
//! let mask = input & 15u32;
//! assert_eq!(mask | 3u32, 7u32);
//! ```

pub mod left {
    type Scalar = u32;

    pub fn first(input: Scalar, gate: bool) -> (Scalar, Scalar, bool) {
        let (low, high) = (input & 15u32, input >> 4u32);
        let dead = 21u32;
        let widened = high | 4u32;
        let shifted = low >> 2u32;
        let copied = shifted;
        let flagged = if gate { true } else { false };
        (widened, copied ^ 3u32, flagged)
    }

    pub fn same(input: Scalar, gate: bool) -> (Scalar, Scalar, bool) {
        ((input >> 4u32) | 4u32, ((input & 15u32) >> 2u32) ^ 3u32, gate)
    }

    pub fn different(input: Scalar, gate: bool) -> (Scalar, Scalar, bool) {
        let (low, high) = (input & 15u32, input >> 4u32);
        let dead = 21u32;
        let widened = high | 4u32;
        let shifted = low >> 2u32;
        let copied = shifted;
        let flagged = if gate { true } else { false };
        (widened, copied | 3u32, flagged)
    }
}

pub mod right {
    pub fn first(input: u32, gate: bool) -> (u32, u32, bool) {
        let (low, high) = (input & 15u32, input >> 4u32);
        let dead = 21u32;
        let widened = high | 4u32;
        let shifted = low >> 2u32;
        let copied = shifted;
        let flagged = if gate { true } else { false };
        (widened, copied ^ 3u32, flagged)
    }

    pub fn same(input: u32, gate: bool) -> (u32, u32, bool) {
        ((input >> 4u32) | 4u32, ((input & 15u32) >> 2u32) ^ 3u32, gate)
    }

    pub fn different(input: u32, gate: bool) -> (u32, u32, bool) {
        let (low, high) = (input & 15u32, input >> 4u32);
        let dead = 21u32;
        let widened = high | 4u32;
        let shifted = low >> 2u32;
        let copied = shifted;
        let flagged = if gate { true } else { false };
        (widened, copied | 3u32, flagged)
    }
}

pub struct Owner;

impl Owner {
    pub fn first(input: u32, gate: bool) -> (u32, u32, bool) {
        let (low, high) = (input & 15u32, input >> 4u32);
        let dead = 21u32;
        let widened = high | 4u32;
        let shifted = low >> 2u32;
        let copied = shifted;
        let flagged = if gate { true } else { false };
        (widened, copied ^ 3u32, flagged)
    }

    pub fn same(input: u32, gate: bool) -> (u32, u32, bool) {
        ((input >> 4u32) | 4u32, ((input & 15u32) >> 2u32) ^ 3u32, gate)
    }

    pub fn different(input: u32, gate: bool) -> (u32, u32, bool) {
        let (low, high) = (input & 15u32, input >> 4u32);
        let dead = 21u32;
        let widened = high | 4u32;
        let shifted = low >> 2u32;
        let copied = shifted;
        let flagged = if gate { true } else { false };
        (widened, copied | 3u32, flagged)
    }
}
