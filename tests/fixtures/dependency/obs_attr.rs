//! Witness attribute exposing observed owner syntax.

use core::str::FromStr;

use proc_macro::{Delimiter, Group, TokenStream, TokenTree};

/// Expose owner syntax through a function trace or owner constant.
#[proc_macro_attribute]
pub fn obs(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut tokens: Vec<TokenTree> = item.into_iter().collect();
    let body_index = tokens
        .iter()
        .rposition(
            |tree| matches!(tree, TokenTree::Group(group) if group.delimiter() == Delimiter::Brace),
        )
        .expect("the observed owner has a body");
    let spelling = match &tokens[body_index] {
        TokenTree::Group(group) => group.to_string(),
        _ => unreachable!(),
    };
    let function = tokens
        .iter()
        .find_map(|tree| match tree {
            TokenTree::Ident(ident) => match ident.to_string().as_str() {
                "fn" => Some(true),
                "mod" | "impl" => Some(false),
                _ => None,
            },
            _ => None,
        })
        .expect("a function, module or implementation owner");
    let injected = if function {
        let escaped = spelling
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('{', "{{")
            .replace('}', "}}");
        format!("println!(\"{escaped}\"); {spelling}")
    } else {
        let TokenTree::Group(body) = &tokens[body_index] else {
            unreachable!()
        };
        format!("{} pub const SPELLING: &str = {spelling:?};", body.stream())
    };
    let new_body = TokenStream::from_str(&injected).expect("the observed body parses");
    tokens[body_index] = TokenTree::Group(Group::new(Delimiter::Brace, new_body));
    TokenStream::from_iter(tokens)
}
