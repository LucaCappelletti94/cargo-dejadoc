#![doc = include_str!("../README.md")]
#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod alpha;
mod drift;
#[cfg(test)]
mod tests;

use alloc::vec::Vec;
use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;

/// The canonical token stream of `file`, equal across formatting drift and local binder names.
#[must_use]
pub fn canonicalize(mut file: syn::File) -> TokenStream {
    drift::normalize_file(&mut file);
    alpha::normalize_file(&mut file);
    fold_tokens(file.to_token_stream(), false)
        .into_iter()
        .collect()
}

/// Drop each group's trailing comma and respell literals outside `opaque` macro and attribute tokens.
fn fold_tokens(stream: TokenStream, opaque: bool) -> Vec<TokenTree> {
    let mut out: Vec<TokenTree> = Vec::new();
    for tree in stream {
        let tree = match tree {
            TokenTree::Group(group) => {
                let shut = opaque
                    || matches!(out.last(), Some(TokenTree::Punct(p)) if matches!(p.as_char(), '!' | '#'));
                drift::map_group(&group, |inner| {
                    let mut inner = fold_tokens(inner, shut);
                    // A one-tuple keeps its comma, the paren fold already told it apart.
                    if matches!(inner.last(), Some(TokenTree::Punct(p)) if p.as_char() == ',') {
                        inner.pop();
                    }
                    inner.into_iter().collect()
                })
            }
            TokenTree::Literal(lit) if !opaque => drift::canonical_literal(lit),
            other => other,
        };
        out.push(tree);
    }
    out
}
