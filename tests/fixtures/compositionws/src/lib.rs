//! ```rust
//! struct S { x: u32, y: bool }
//! impl S { fn one() -> u32 { 1 } }
//! impl S { fn two() -> u32 { 2 } }
//! fn make(input: u32, p: bool) -> (u32, bool) {
//!     let a = input & 1u32;
//!     let b = input ^ 1u32;
//!     let s = S { x: b, y: !!p };
//!     (s.x, s.y)
//! }
//! fn main() { println!("{:?}", (S::one(), S::two(), make(5, true))); }
//! ```
//!
//! ```rust
//! fn main() { println!("{:?}", (S::one(), S::two(), make(5, true))); }
//! struct S { x: u32, y: bool }
//! fn make(input: u32, p: bool) -> (u32, bool) {
//!     let b = input ^ 1u32;
//!     let a = input & 1u32;
//!     let s = S { y: p, x: b };
//!     (s.x, s.y)
//! }
//! impl S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } }
//! ```

pub mod literal_and_boolean {
    pub fn first(input: u32, p: bool) -> (bool, bool) {
        let m = match input {
            1 | 2 => true,
            _ => false,
        };
        (!!p, m)
    }

    pub fn same(input: u32, p: bool) -> (bool, bool) {
        let m = match input {
            2 | 1 => true,
            _ => false,
        };
        (p, m)
    }

    pub fn different(input: u32, p: bool) -> (bool, bool) {
        let m = match input {
            2 | 1 if input != 0 => true,
            _ => false,
        };
        (p, m)
    }
}

pub mod helpers_and_impls {
    fn low(x: u32) -> u32 {
        x & 15u32
    }

    pub struct T;

    impl T {
        pub fn one(input: u32) -> u32 {
            low(input) ^ 1u32
        }
    }

    impl T {
        pub fn two() -> u32 {
            2
        }
    }

    pub fn first(input: u32) -> (u32, u32) {
        (T::one(input), T::two())
    }

    pub fn same(input: u32) -> (u32, u32) {
        (T::one(input), T::two())
    }

    pub fn different(input: u32) -> (u32, u32) {
        (T::one(input ^ 1u32), T::two())
    }
}
