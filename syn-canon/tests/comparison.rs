//! Opaque macro structure in canonical comparisons.

use proc_macro2::{Delimiter, Group};

fn observed(grouped: bool) -> syn::File {
    let mut file: syn::File = syn::parse_str("fn main() { inspect!(value); }").unwrap();
    let syn::Item::Fn(function) = &mut file.items[0] else {
        panic!("function fixture");
    };
    let syn::Stmt::Macro(statement) = &mut function.block.stmts[0] else {
        panic!("macro fixture");
    };
    if grouped {
        statement.mac.tokens = core::iter::once(proc_macro2::TokenTree::Group(Group::new(
            Delimiter::None,
            statement.mac.tokens.clone(),
        )))
        .collect();
    }
    file
}

#[test]
fn invisible_macro_groups_preserve_observable_structure() {
    assert_ne!(
        syn_canon::canonicalize(observed(false)).to_string(),
        syn_canon::canonicalize(observed(true)).to_string(),
    );
    assert_ne!(
        syn_canon::canonicalize_failing(observed(false)).to_string(),
        syn_canon::canonicalize_failing(observed(true)).to_string(),
    );
}
