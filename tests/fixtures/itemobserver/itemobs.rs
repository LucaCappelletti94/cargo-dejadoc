//! Procedural observers expose original item payloads and source declaration order.

use core::str::FromStr;

use proc_macro::{Delimiter, Group, TokenStream, TokenTree};

/// Record the payload of an observed function or module.
#[proc_macro_attribute]
pub fn itemobs(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return TokenStream::from_str("compile_error!(\"itemobs takes no arguments\");")
            .expect("a literal error message parses");
    }
    let tokens: Vec<TokenTree> = item.into_iter().collect();
    let head = tokens.first().map(|tree| tree.to_string());
    match head.as_deref() {
        Some("fn") => observe_function(tokens),
        Some("mod") => observe_module(tokens),
        Some("impl") => TokenStream::from_str(
            "compile_error!(\"itemobs does not observe implementations\");",
        )
        .expect("a literal error message parses"),
        _ => TokenStream::from_str("compile_error!(\"itemobs observes functions and modules\");")
            .expect("a literal error message parses"),
    }
}

fn body_index(tokens: &[TokenTree]) -> Option<usize> {
    tokens.iter().rposition(|tree| {
        matches!(
            tree,
            TokenTree::Group(group) if group.delimiter() == Delimiter::Brace
        )
    })
}

fn observe_function(tokens: Vec<TokenTree>) -> TokenStream {
    let index = match body_index(&tokens) {
        Some(index) => index,
        None => {
            return TokenStream::from_str("compile_error!(\"itemobs needs a function body\");")
                .expect("a literal error message parses")
        }
    };
    let TokenTree::Group(body) = &tokens[index] else {
        unreachable!()
    };
    let payload = body.stream().to_string();
    let injected = format!(
        "println!(\"{{}}\", \"{}\"); {}",
        payload.escape_debug(),
        body.stream()
    );
    let mut new_tokens = tokens;
    new_tokens[index] = TokenTree::Group(Group::new(
        Delimiter::Brace,
        TokenStream::from_str(&injected).expect("the observed function body parses"),
    ));
    TokenStream::from_iter(new_tokens)
}

fn observe_module(tokens: Vec<TokenTree>) -> TokenStream {
    let index = match body_index(&tokens) {
        Some(index) => index,
        None => {
            return TokenStream::from_str("compile_error!(\"itemobs needs a module body\");")
                .expect("a literal error message parses")
        }
    };
    let TokenTree::Group(body) = &tokens[index] else {
        unreachable!()
    };
    let payload = body.stream().to_string();
    let injected = format!(
        "pub const SPELLING: &str = \"{}\"; {}",
        payload.escape_debug(),
        body.stream()
    );
    let mut new_tokens = tokens;
    new_tokens[index] = TokenTree::Group(Group::new(
        Delimiter::Brace,
        TokenStream::from_str(&injected).expect("the observed module body parses"),
    ));
    TokenStream::from_iter(new_tokens)
}

#[proc_macro]
pub fn order(_: TokenStream) -> TokenStream {
    TokenStream::from_str("{ let source = include_str!(file!()); source.find(\"fn one\").unwrap() < source.find(\"fn two\").unwrap() }")
        .expect("the source order expression parses")
}

#[proc_macro]
pub fn println(args: TokenStream) -> TokenStream {
    let expression = order(args);
    TokenStream::from_str(&format!("std::println!(\"{{}}\", {expression})"))
        .expect("the source order observation parses")
}
