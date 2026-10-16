//! ```rust
//! fn low(x:u32)->u32{x&15u32}
//! fn high(x:u32)->u32{x>>4}
//! fn first(input:u32)->(u32,u32){let a=low(input);let b=high(input);(a,b)}
//! ```
//!
//! ```rust
//! fn low(x:u32)->u32{x&15u32}
//! fn high(x:u32)->u32{x>>4}
//! fn second(input:u32)->(u32,u32){let b=high(input);let a=low(input);(a,b)}
//! ```
//!
//! ```compile_fail
//! fn low(x:u32)->u32{x&15u32}
//! fn high(x:u32)->u32{x>>4}
//! fn first(input:u32)->(u32,u32){let a=low(input);let b=high(input);(a,b)}
//! ```
//!
//! ```compile_fail
//! fn low(x:u32)->u32{x&15u32}
//! fn high(x:u32)->u32{x>>4}
//! fn second(input:u32)->(u32,u32){let b=high(input);let a=low(input);(a,b)}
//! ```

pub mod left {
    type Scalar = u32;
    fn low(x: Scalar) -> Scalar { if x < 16u32 { x & 15u32 } else { let value = x & 7u32; value } }
    fn shift(x: Scalar) -> Scalar { x >> 4 }
    fn high(x: Scalar) -> Scalar { shift(x) }
    pub fn first(input: Scalar) -> (Scalar, Scalar) { let dead = 42u32; let a = low(input); let b = high(input); (a, b) }
    pub fn same(input: Scalar) -> (Scalar, Scalar) { let b = high(input); let a = low(input); (a, b) }
    pub fn changed(input: Scalar) -> (Scalar, Scalar) { let a = low(input); let b = low(input); (a, b) }
}

pub mod right {
    use core::primitive::u64 as Scalar;
    fn r#type(x: Scalar) -> Scalar { x & 15u64 }
    fn shift(x: Scalar) -> Scalar { x >> 4 }
    fn high(x: Scalar) -> Scalar { shift(x) }
    pub fn first(input: Scalar) -> (Scalar, Scalar) { let a = r#type(input); let b = high(input); (a, b) }
    pub fn same(input: Scalar) -> (Scalar, Scalar) { let b = high(input); let a = r#type(input); (a, b) }
    pub fn changed(input: Scalar) -> (Scalar, Scalar) { let a = r#type(input); let b = r#type(input); (a, b) }
}

pub mod caller {
    #[track_caller]
    fn observed(x: u32) -> u32 { x & 15u32 }
    pub fn first(input: u32) -> (u32, u32) { let a = observed(input); let b = observed(input ^ 1u32); (a, b) }
    pub fn same(input: u32) -> (u32, u32) { let b = observed(input ^ 1u32); let a = observed(input); (a, b) }
}

pub mod shadow {
    fn helper(x: u32) -> u32 { x & 15u32 }
    pub fn first(input: u32) -> (u32, u32) { let helper = |x: u32| { println!("{x}"); x }; let a = helper(input); let b = helper(input ^ 1u32); (a, b) }
    pub fn same(input: u32) -> (u32, u32) { let helper = |x: u32| { println!("{x}"); x }; let b = helper(input ^ 1u32); let a = helper(input); (a, b) }
}
