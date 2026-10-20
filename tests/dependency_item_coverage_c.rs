//! Macro-argument rewriter arms, impl-member name fallback and group-wrapped
//! context locals.

#[path = "support/grouped_file.rs"]
mod grouped;

use grouped::grouped_file;

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_file(source).unwrap())
}

fn failing(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize_failing(syn::parse_file(source).unwrap())
}

/// Canonicalize a hand-built file.
fn file_form(file: syn::File) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(file)
}

/// A struct plus one inherent impl whose members carry the given source order,
/// with a trailing verbatim member so the impl member name fallback applies.
fn impl_with_verbatim(members: &str) -> syn::File {
    let mut file: syn::File = syn::parse_str(&format!("struct S; impl S {{ {members} }}")).unwrap();
    for item in &mut file.items {
        if let syn::Item::Impl(impl_) = item {
            impl_
                .items
                .push(syn::ImplItem::Verbatim("42".parse().unwrap()));
        }
    }
    file
}

#[test]
fn format_string_brace_escapes_fold_like_renamed_captures() {
    let left = "fn one() -> u32 { 1 } fn f() { let x = 1u32; let _ = println!(\"{{ {x} }}\", x); }";
    let right =
        "fn one() -> u32 { 1 } fn f() { let y = 1u32; let _ = println!(\"{{ {y} }}\", y); }";
    let control =
        "fn one() -> u32 { 1 } fn f() { let x = 1u32; let _ = println!(\"{{ {x} + 1 }}\", x); }";
    assert_eq!(form(left), form(right));
    assert_ne!(form(left), form(control));
}

#[test]
fn bare_qself_paths_in_format_arguments_fold() {
    let left = "fn one() -> u32 { 1 } fn f() { let x = 1u32; let _ = println!(\"{}\", <u32>::MAX); let _ = x; }";
    let right = "fn one() -> u32 { 1 } fn f() { let y = 1u32; let _ = println!(\"{}\", <u32>::MAX); let _ = y; }";
    let control = "fn one() -> u32 { 1 } fn f() { let x = 1u32; let _ = println!(\"{}\", <u32>::MIN); let _ = x; }";
    assert_eq!(form(left), form(right));
    assert_ne!(form(left), form(control));
}

#[test]
fn match_arms_in_format_arguments_fold() {
    let left = "fn one() -> u32 { 1 } fn f() { let x = 1u32; let _ = println!(\"{}\", { match x { 1 => 2, _ => 3 } }); }";
    let right = "fn one() -> u32 { 1 } fn f() { let y = 1u32; let _ = println!(\"{}\", { match y { 1 => 2, _ => 3 } }); }";
    let control = "fn one() -> u32 { 1 } fn f() { let x = 1u32; let _ = println!(\"{}\", { match x { 1 => 5, _ => 3 } }); }";
    assert_eq!(form(left), form(right));
    assert_ne!(form(left), form(control));
}

#[test]
fn if_arms_without_else_in_format_arguments_fold() {
    let left = "fn one() -> u32 { 1 } fn f() { let b = true; let _ = println!(\"{}\", { if b { 1 } 2 }); }";
    let right = "fn one() -> u32 { 1 } fn f() { let c = true; let _ = println!(\"{}\", { if c { 1 } 2 }); }";
    let control = "fn one() -> u32 { 1 } fn f() { let b = true; let _ = println!(\"{}\", { if b { 4 } 2 }); }";
    assert_eq!(form(left), form(right));
    assert_ne!(form(left), form(control));
}

#[test]
fn uninitialized_locals_in_format_arguments_fold_in_a_region() {
    let left = "fn one() -> u32 { 1 } fn f() { let _ = println!(\"{}\", { let x: u32; let _ = x + 1; 3 }); }";
    let right = "fn one() -> u32 { 1 } fn f() { let _ = println!(\"{}\", { let y: u32; let _ = y + 1; 3 }); }";
    let control = "fn one() -> u32 { 1 } fn f() { let _ = println!(\"{}\", { let x: u32; let _ = x + 2; 3 }); }";
    assert_eq!(form(left), form(right));
    assert_ne!(form(left), form(control));
}

#[test]
fn bare_path_attributes_in_format_arguments_fold() {
    let left = "fn one() -> u32 { 1 } fn f() { let _ = println!(\"{}\", { #[a] fn g() -> u32 { 2 } g() }); }";
    let right = "fn one() -> u32 { 1 } fn f() { let _ = println!(\"{}\", { #[a] fn h() -> u32 { 2 } h() }); }";
    let control = "fn one() -> u32 { 1 } fn f() { let _ = println!(\"{}\", { #[b] fn g() -> u32 { 2 } g() }); }";
    assert_eq!(form(left), form(right));
    assert_ne!(form(left), form(control));
}

#[test]
fn impl_regions_with_verbatim_members_keep_the_member_order() {
    assert_ne!(
        file_form(impl_with_verbatim(
            "fn b() -> u32 { 2 } fn a() -> u32 { 1 }"
        )),
        file_form(impl_with_verbatim(
            "fn a() -> u32 { 1 } fn b() -> u32 { 2 }"
        ))
    );
    let plain_ordered = "struct S; impl S { fn b() -> u32 { 2 } fn a() -> u32 { 1 } }";
    let plain_reversed = "struct S; impl S { fn a() -> u32 { 1 } fn b() -> u32 { 2 } }";
    assert_eq!(form(plain_ordered), form(plain_reversed));
    assert_ne!(failing(plain_ordered), failing(plain_reversed));
}

#[test]
fn group_wrapped_context_locals_fold_like_renamed_scalars() {
    let left = file_form(grouped_file(
        "fn one() -> u32 { 1 } fn two() -> u32 { 2 } trait T { fn m(&self); } fn ctx(p: u32) { let a = p + 1; let _ = a; }",
    ));
    let right = file_form(grouped_file(
        "fn one() -> u32 { 1 } fn two() -> u32 { 2 } trait T { fn m(&self); } fn ctx(p: u32) { let b = p + 1; let _ = b; }",
    ));
    let control = file_form(grouped_file(
        "fn one() -> u32 { 1 } fn two() -> u32 { 2 } trait T { fn m(&self); } fn ctx(p: u32) { let a = p + 2; let _ = a; }",
    ));
    assert_eq!(left, right);
    assert_ne!(left, control);
}
