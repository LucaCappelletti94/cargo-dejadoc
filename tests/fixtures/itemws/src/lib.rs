//! ```rust
//! fn one(input: u32) -> (u32, u32, u32, u32) { ((input & 15u32) ^ 3u32, (input >> 4u32) | 4u32, (input & 7u32) ^ 1u32, input) }
//! fn two(input: u32) -> (u32, u32, u32, u32) { ((input | 4u32) ^ 3u32, (input & 15u32) >> 2u32, (input | 8u32) & 7u32, input) }
//! fn main() { println!("{:?}", (one(1u32), two(2u32), one(3u32))); }
//! ```
//!
//! ```rust
//! fn two(input: u32) -> (u32, u32, u32, u32) { ((input | 4u32) ^ 3u32, (input & 15u32) >> 2u32, (input | 8u32) & 7u32, input) }
//!
//! fn one(input: u32) -> (u32, u32, u32, u32) { ((input & 15u32) ^ 3u32, (input >> 4u32) | 4u32, (input & 7u32) ^ 1u32, input) }
//!
//! fn main() { println!("{:?}", (one(1u32), two(2u32), one(3u32))); }
//! ```
//!
//! ```rust
//! struct S;
//! impl S { fn one(input: u32) -> (u32, u32, u32, u32) { ((input & 15u32) ^ 3u32, (input >> 4u32) | 4u32, (input & 7u32) ^ 1u32, input) } }
//! impl S { fn two(input: u32) -> (u32, u32, u32, u32) { ((input | 4u32) ^ 3u32, (input & 15u32) >> 2u32, (input | 8u32) & 7u32, input) } }
//! fn main() { println!("{:?}", (S::one(1u32), S::two(2u32), S::one(3u32))); }
//! ```
//!
//! ```rust
//! struct S;
//! impl S {
//!     fn one(input: u32) -> (u32, u32, u32, u32) { ((input & 15u32) ^ 3u32, (input >> 4u32) | 4u32, (input & 7u32) ^ 1u32, input) }
//!     fn two(input: u32) -> (u32, u32, u32, u32) { ((input | 4u32) ^ 3u32, (input & 15u32) >> 2u32, (input | 8u32) & 7u32, input) }
//! }
//!
//! fn main() { println!("{:?}", (S::one(1u32), S::two(2u32), S::one(3u32))); }
//! ```
//!
//! ```rust
//! struct S;
//! impl S { fn one(input: u32) -> (u32, u32, u32, u32) { ((input & 15u32) ^ 3u32, (input >> 4u32) | 4u32, (input & 7u32) ^ 1u32, input) } fn two(input: u32) -> (u32, u32, u32, u32) { ((input | 4u32) ^ 4u32, (input & 15u32) >> 2u32, (input | 8u32) & 7u32, input) } }
//! fn main() { println!("{:?}", (S::one(1u32), S::two(2u32), S::one(3u32))); }
//! ```

pub mod left {
    pub fn first(input: u32, gate: bool) -> (u32, u32, bool) {
        fn one(value: u32) -> u32 { ((value & 15u32) ^ 3u32) | ((value >> 4u32) & 31u32) }
        fn two(value: u32) -> u32 { ((value | 4u32) ^ 3u32) & ((value >> 2u32) | 7u32) }
        let copied = input;
        (one(copied), two(input), if gate { true } else { false })
    }

    pub fn same(input: u32, gate: bool) -> (u32, u32, bool) {
        fn two(value: u32) -> u32 { ((value | 4u32) ^ 3u32) & ((value >> 2u32) | 7u32) }
        fn one(value: u32) -> u32 { ((value & 15u32) ^ 3u32) | ((value >> 4u32) & 31u32) }
        (one(input), two(input), gate)
    }

    pub fn different(input: u32, gate: bool) -> (u32, u32, bool) {
        fn one(value: u32) -> u32 { ((value & 15u32) ^ 4u32) | ((value >> 4u32) & 31u32) }
        fn two(value: u32) -> u32 { ((value | 4u32) ^ 3u32) & ((value >> 2u32) | 7u32) }
        (one(input), two(input), gate)
    }
}

pub mod right {
    pub fn first(input: u32, gate: bool) -> (u32, u32, bool) {
        struct Local;
        impl Local {
            fn one(value: u32) -> u32 { ((value & 15u32) ^ 3u32) | ((value >> 4u32) & 31u32) }
        }
        impl Local {
            fn two(value: u32) -> u32 { ((value | 4u32) ^ 3u32) & ((value >> 2u32) | 7u32) }
        }
        (Local::one(input), Local::two(input), gate)
    }

    pub fn same(input: u32, gate: bool) -> (u32, u32, bool) {
        struct Local;
        impl Local {
            fn two(value: u32) -> u32 { ((value | 4u32) ^ 3u32) & ((value >> 2u32) | 7u32) }
            fn one(value: u32) -> u32 { ((value & 15u32) ^ 3u32) | ((value >> 4u32) & 31u32) }
        }
        (Local::one(input), Local::two(input), gate)
    }

    pub fn different(input: u32, gate: bool) -> (u32, u32, bool) {
        struct Local;
        impl Local {
            fn one(value: u32) -> u32 { ((value & 15u32) ^ 4u32) | ((value >> 4u32) & 31u32) }
            fn two(value: u32) -> u32 { ((value | 4u32) ^ 3u32) & ((value >> 2u32) | 7u32) }
        }
        (Local::one(input), Local::two(input), gate)
    }
}

pub struct Owner;

impl Owner {
    pub fn first(input: u32, gate: bool) -> (u32, u32, bool) {
        fn one(value: u32) -> u32 { ((value & 15u32) ^ 3u32) | ((value >> 4u32) & 31u32) }
        fn two(value: u32) -> u32 { ((value | 4u32) ^ 3u32) & ((value >> 2u32) | 7u32) }
        (one(input), two(input), gate)
    }

    pub fn same(input: u32, gate: bool) -> (u32, u32, bool) {
        fn two(value: u32) -> u32 { ((value | 4u32) ^ 3u32) & ((value >> 2u32) | 7u32) }
        fn one(value: u32) -> u32 { ((value & 15u32) ^ 3u32) | ((value >> 4u32) & 31u32) }
        (one(input), two(input), gate)
    }
}

impl Owner {
    pub fn different(input: u32, gate: bool) -> (u32, u32, bool) {
        fn one(value: u32) -> u32 { ((value & 15u32) ^ 4u32) | ((value >> 4u32) & 31u32) }
        fn two(value: u32) -> u32 { ((value | 4u32) ^ 3u32) & ((value >> 2u32) | 7u32) }
        (one(input), two(input), gate)
    }
}
