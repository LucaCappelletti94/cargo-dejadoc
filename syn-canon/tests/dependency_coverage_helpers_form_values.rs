//! Hand-built empty and punctuated paths in canonical comparisons.

const CALLER: &str = "fn h(x: u32) -> u32 { x & 15 } fn f(x: u32) -> u32 { h(x) }";

fn call_path_in_f(file: &mut syn::File) -> &mut syn::Path {
    let syn::Item::Fn(caller) = &mut file.items[1] else {
        unreachable!("the second item is the caller")
    };
    let syn::Stmt::Expr(expr, _) = &mut caller.block.stmts[0] else {
        unreachable!("the caller body is one call")
    };
    let syn::Expr::Call(call) = expr else {
        unreachable!("the statement is a call")
    };
    let syn::Expr::Path(path) = call.func.as_mut() else {
        unreachable!("the callee is a path")
    };
    &mut path.path
}

#[test]
fn trailing_path_separators_keep_helper_call_forms_distinct() {
    let plain: syn::File = syn::parse_str(CALLER).unwrap();
    let mut punctuated: syn::File = syn::parse_str(CALLER).unwrap();
    call_path_in_f(&mut punctuated)
        .segments
        .push_punct(syn::token::PathSep::default());
    assert_ne!(
        syn_canon::canonicalize(plain),
        syn_canon::canonicalize(punctuated)
    );
}

#[test]
fn cleared_call_paths_keep_helper_calls_distinct() {
    let plain: syn::File = syn::parse_str(CALLER).unwrap();
    let mut cleared: syn::File = syn::parse_str(CALLER).unwrap();
    call_path_in_f(&mut cleared).segments.clear();
    assert_ne!(
        syn_canon::canonicalize(plain),
        syn_canon::canonicalize(cleared)
    );
}

fn cleared_macro_in(name: &str) -> syn::File {
    let mut file: syn::File = syn::parse_str(&format!(
        "mod m {{ pub fn f(x: u32) -> u32 {{ x & 15 }} }} \
         fn a() -> u32 {{ {name}!(); m::f(1) }} \
         fn b() -> u32 {{ u32::from(2) }}"
    ))
    .unwrap();
    let index = file
        .items
        .iter()
        .position(|item| match item {
            syn::Item::Fn(function) => function.sig.ident == "a",
            _ => false,
        })
        .expect("the caller item is present");
    let syn::Item::Fn(caller) = &mut file.items[index] else {
        unreachable!("the caller item is a function")
    };
    let syn::Stmt::Macro(mac) = &mut caller.block.stmts[0] else {
        unreachable!("the first statement is the macro")
    };
    mac.mac.path.segments.clear();
    file
}

#[test]
fn empty_path_macros_fold_across_original_names() {
    let first = cleared_macro_in("foo");
    let second = cleared_macro_in("bar");
    assert_eq!(
        syn_canon::canonicalize(first),
        syn_canon::canonicalize(second)
    );
}

#[test]
fn grouped_local_helper_calls_fold_across_item_orderings() {
    let first: syn::File = syn::parse_str(
        "fn a() -> u32 { mod m { fn f(x: u32) -> u32 { x & 15 } } m::f(1) } \
         fn b() -> u32 { mod n { fn g(x: u32) -> u32 { x & 15 } } n::g(2) }",
    )
    .unwrap();
    let second: syn::File = syn::parse_str(
        "fn b() -> u32 { mod n { fn g(x: u32) -> u32 { x & 15 } } n::g(2) } \
         fn a() -> u32 { mod m { fn f(x: u32) -> u32 { x & 15 } } m::f(1) }",
    )
    .unwrap();
    assert_eq!(
        syn_canon::canonicalize(first),
        syn_canon::canonicalize(second)
    );
}

#[test]
fn println_statements_fold_across_item_orderings() {
    let first: syn::File =
        syn::parse_str("fn a() -> u32 { println!(\"{}\", b); 1 } fn b() -> u32 { 2 }").unwrap();
    let second: syn::File =
        syn::parse_str("fn b() -> u32 { 2 } fn a() -> u32 { println!(\"{}\", b); 1 }").unwrap();
    assert_eq!(
        syn_canon::canonicalize(first),
        syn_canon::canonicalize(second)
    );
}

#[test]
fn grouped_helper_calls_fold_across_item_orderings() {
    let a: syn::File = syn::parse_str(
        "mod m { pub fn f(x: u32) -> u32 { x & 15 } } fn a() -> u32 { m::f(1) } fn b() -> u32 { m::f(2) }",
    )
    .unwrap();
    let b: syn::File = syn::parse_str(
        "mod m { pub fn f(x: u32) -> u32 { x & 15 } } fn b() -> u32 { m::f(2) } fn a() -> u32 { m::f(1) }",
    )
    .unwrap();
    assert_eq!(syn_canon::canonicalize(a), syn_canon::canonicalize(b));
}
