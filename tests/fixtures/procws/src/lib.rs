//! Two exported procedural macros with one body, public API the dogfood job pins.

use proc_macro::TokenStream;

/// Returns its input unchanged.
#[proc_macro]
pub fn first(input: TokenStream) -> TokenStream {
    let tokens = input.into_iter().collect::<TokenStream>();
    tokens
}

/// Returns its input unchanged under another name.
#[proc_macro]
pub fn second(input: TokenStream) -> TokenStream {
    let tokens = input.into_iter().collect::<TokenStream>();
    tokens
}
