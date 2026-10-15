//! Macro argument provenance in closed item groups.

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_file(source).unwrap())
}

fn failing(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize_failing(syn::parse_file(source).unwrap())
}

fn folded(left: &str, right: &str) {
    assert_eq!(form(left), form(right), "{left}\n{right}");
    assert_ne!(failing(left), failing(right), "{left}\n{right}");
}

fn distinct(left: &str, right: &str) {
    assert_ne!(form(left), form(right), "{left}\n{right}");
    assert_ne!(failing(left), failing(right), "{left}\n{right}");
}

#[test]
fn item_macros_fold_implicit_capture_references() {
    folded(
        "const A: u32 = 1; const B: u32 = 2; fn f() { println!(\"{A}\"); } fn g() { println!(\"{B}\"); }",
        "const B: u32 = 2; const A: u32 = 1; fn g() { println!(\"{B}\"); } fn f() { println!(\"{A}\"); }",
    );
}

#[test]
fn item_macros_fold_positional_and_named_arguments() {
    folded(
        "const A: u32 = 1; const B: u32 = 2; fn f() { let c = 3; println!(\"{A} {} {B}\", c); }",
        "const B: u32 = 2; const A: u32 = 1; fn f() { let c = 3; println!(\"{A} {} {B}\", c); }",
    );
}

#[test]
fn item_macros_fold_width_and_precision_references() {
    folded(
        "const W: usize = 8; const P: usize = 2; const V: u32 = 1; fn f() { println!(\"{V: W$.P$}\"); }",
        "const Q: usize = 2; const X: usize = 8; const V: u32 = 1; fn f() { println!(\"{V: X$.Q$}\"); }",
    );
}

#[test]
fn item_macros_keep_different_width_origins_separate() {
    distinct(
        "const W: usize = 8; const U: usize = 24; fn f() { let v = 1; println!(\"{v: W$}\"); }",
        "const W: usize = 8; const U: usize = 24; fn f() { let v = 1; println!(\"{v: U$}\"); }",
    );
}

#[test]
fn item_macros_fold_nested_format_references() {
    folded(
        "const A: u32 = 1; const B: u32 = 2; fn f() { println!(\"{A} {}\", format!(\"{B}\")); }",
        "const B: u32 = 2; const A: u32 = 1; fn f() { println!(\"{A} {}\", format!(\"{B}\")); }",
    );
}

#[test]
fn item_macros_rebind_generic_arguments() {
    assert_eq!(
        form(
            "fn one<T>() { let t = 1; let w = 4; println!(\"{t: w$} {}\", core::mem::size_of::<T>()); }"
        ),
        form(
            "fn two<U>() { let u = 1; let w = 4; println!(\"{u: w$} {}\", core::mem::size_of::<U>()); }"
        ),
    );
}

#[test]
fn item_macros_freeze_unparsable_argument_groups() {
    distinct(
        "fn one() -> u32 { 1 } fn two() -> u32 { 2 } fn main() { println!(; 3); }",
        "fn two() -> u32 { 2 } fn one() -> u32 { 1 } fn main() { println!(; 3); }",
    );
}

#[test]
fn item_macros_fold_nested_format_capture_names() {
    folded(
        "const A: u32 = 1; const B: u32 = 2; fn f() { let p = 3; println!(\"{} {}\", format!(\"{A} {B}\"), p); }",
        "const D: u32 = 2; const C: u32 = 1; fn g() { let q = 3; println!(\"{} {}\", format!(\"{C} {D}\"), q); }",
    );
}

#[test]
fn item_macros_keep_nested_format_origins_separate() {
    distinct(
        "const A: u32 = 1; const B: u32 = 2; fn f() { println!(\"{}\", format!(\"{A}\")); }",
        "const A: u32 = 1; const B: u32 = 2; fn g() { println!(\"{}\", format!(\"{B}\")); }",
    );
}

#[test]
fn item_macros_rebind_raw_generic_arguments() {
    assert_eq!(
        form(
            "struct S<T>(T); impl<r#type> S<r#type> { fn one() { println!(\"{}\", core::mem::size_of::<r#type>()); } } impl<U> S<U> { fn two() { println!(\"{}\", core::mem::size_of::<U>()); } }"
        ),
        form(
            "struct S<T>(T); impl<V> S<V> { fn one() { println!(\"{}\", core::mem::size_of::<V>()); } } impl<W> S<W> { fn two() { println!(\"{}\", core::mem::size_of::<W>()); } }"
        ),
    );
}

#[test]
fn nested_unknown_macros_keep_ancestor_item_groups_atomic() {
    distinct(
        "fn one() -> u32 { 1 } fn two() -> u32 { 2 } fn main() { println!(\"{:?}\", observer::order!((one(), two()))); }",
        "fn two() -> u32 { 2 } fn one() -> u32 { 1 } fn main() { println!(\"{:?}\", observer::order!((one(), two()))); }",
    );
}
