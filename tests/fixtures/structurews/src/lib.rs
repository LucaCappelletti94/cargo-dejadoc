//! ```
//! struct Point { x: u32, y: u32 }
//! fn main() {
//!     let point = Point { x: 48u32 & 15u32, y: 48u32 >> 4 };
//!     assert_eq!((point.x, point.y), (0, 3));
//! }
//! ```
//!
//! ```
//! struct Point { x: u32, y: u32 }
//! fn main() {
//!     let point = Point { y: 48u32 >> 4, x: 48u32 & 15u32 };
//!     assert_eq!((point.x, point.y), (0, 3));
//! }
//! ```
//!
//! ```
//! struct Point { x: u32, y: u32 }
//! fn main() {
//!     let point = Point { x: 48u32 & 15u32, ..Point { x: 0, y: 48u32 >> 4 } };
//!     assert_eq!((point.x, point.y), (0, 3));
//! }
//! ```
//!
//! ```rust,should_panic
//! struct Point { x: u32, y: u32 }
//! fn main() {
//!     let point = Point { y: 48u32 >> 4, x: 48u32 & 15u32 };
//!     assert_eq!((point.x, point.y), (16, 3));
//! }
//! ```

pub mod fields {
    pub struct Point {
        pub x: u32,
        pub y: u32,
    }

    pub fn first(input: u32) -> (u32, u32) {
        let x = input & 15;
        let y = input >> 4;
        let point = Point { x, y };
        (point.x, point.y)
    }

    pub fn same(input: u32) -> (u32, u32) {
        let x = input & 15;
        let y = input >> 4;
        let point = Point { y, x };
        (point.x, point.y)
    }

    pub fn different(input: u32) -> (u32, u32) {
        let point = Point {
            x: input & 15,
            ..Point {
                x: 0,
                y: input >> 4,
            }
        };
        (point.x, point.y)
    }
}

pub mod arms {
    pub fn first(input: u32) -> bool {
        match input {
            1 | 2 => true,
            _ => false,
        }
    }

    pub fn same(input: u32) -> bool {
        match input {
            2 | 1 => true,
            _ => false,
        }
    }

    pub fn different(input: u32) -> bool {
        match input {
            2 | 1 if input != 0 => true,
            _ => false,
        }
    }
}
