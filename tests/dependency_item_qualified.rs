//! Qualified owners, import-target aliases, and crate-root proof regressions.

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
fn qualified_owner_impl_blocks_fold_through_module_frame() {
    folded(
        "mod m { pub struct S; } impl m::S { fn one() -> u32 { 1 } } impl m::S { fn two() -> u32 { 2 } } fn main() { println!(\"{:?}\", (m::S::one(), m::S::two())); }",
        "mod m { pub struct S; } impl m::S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } } fn main() { println!(\"{:?}\", (m::S::one(), m::S::two())); }",
    );
    folded(
        "mod m { pub struct S; } impl m::S { fn one() -> u32 { 1 } } impl m::S { fn two() -> u32 { 2 } } fn main() { println!(\"{:?}\", (m::S::one(), m::S::two())); }",
        "mod m { pub struct S; } impl m::S { fn two() -> u32 { 2 } } impl m::S { fn one() -> u32 { 1 } } fn main() { println!(\"{:?}\", (m::S::one(), m::S::two())); }",
    );
}

#[test]
fn qualified_owner_alias_impl_blocks_fold_through_import_target() {
    folded(
        "mod m { pub struct S; } use m::S as A; impl A { fn one() -> u32 { 1 } } impl A { fn two() -> u32 { 2 } } fn main() { println!(\"{:?}\", (A::one(), A::two())); }",
        "mod m { pub struct S; } use m::S as A; impl A { fn one() -> u32 { 1 } fn two() -> u32 { 2 } } fn main() { println!(\"{:?}\", (A::one(), A::two())); }",
    );
    folded(
        "mod m { pub struct S; } use m::S as A; impl A { fn one() -> u32 { 1 } } impl A { fn two() -> u32 { 2 } } fn main() { println!(\"{:?}\", (A::one(), A::two())); }",
        "mod m { pub struct S; } use m::S as A; impl A { fn two() -> u32 { 2 } } impl A { fn one() -> u32 { 1 } } fn main() { println!(\"{:?}\", (A::one(), A::two())); }",
    );
}

#[test]
fn nested_qualified_owner_impl_blocks_fold_through_module_frames() {
    folded(
        "mod m { pub mod n { pub struct S; } } impl m::n::S { fn one() -> u32 { 1 } } impl m::n::S { fn two() -> u32 { 2 } } fn main() { println!(\"{:?}\", (m::n::S::one(), m::n::S::two())); }",
        "mod m { pub mod n { pub struct S; } } impl m::n::S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } } fn main() { println!(\"{:?}\", (m::n::S::one(), m::n::S::two())); }",
    );
}

#[test]
fn qualified_owner_keeps_different_terminals_separate() {
    distinct(
        "mod m { pub struct S; pub struct T; } impl m::S { fn one() -> u32 { 1 } }",
        "mod m { pub struct S; pub struct T; } impl m::T { fn one() -> u32 { 1 } }",
    );
}

#[test]
fn qualified_owner_keeps_different_alias_targets_separate() {
    distinct(
        "mod m { pub struct S; pub struct T; } use m::S as A; impl A { fn one() -> u32 { 1 } }",
        "mod m { pub struct S; pub struct T; } use m::T as A; impl A { fn one() -> u32 { 1 } }",
    );
}

#[test]
fn qualified_owner_self_value_calls_fold_through_module_frame() {
    folded(
        "mod m { pub struct S; } impl m::S { fn one() -> u32 { Self::two() } } impl m::S { fn two() -> u32 { 2 } }",
        "mod m { pub struct S; } impl m::S { fn two() -> u32 { 2 } } impl m::S { fn one() -> u32 { Self::two() } }",
    );
}

#[test]
fn qualified_owner_mixed_spellings_fold_through_one_route() {
    folded(
        "mod m { pub struct S; } use m::S as A; impl m::S { fn one() -> u32 { 1 } } impl A { fn two() -> u32 { 2 } }",
        "mod m { pub struct S; } use m::S as A; impl m::S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } }",
    );
}

#[test]
fn qualified_owner_self_keeps_module_shadow_separate() {
    distinct(
        "struct S; mod m { pub struct S; } impl m::S { fn f() -> u32 { Self::g() } } impl m::S { fn g() -> u32 { 1 } }",
        "struct S; mod m { pub struct S; } impl S { fn f() -> u32 { Self::g() } } impl S { fn g() -> u32 { 1 } }",
    );
}

#[test]
fn qualified_built_in_print_macro_folds_when_unshadowed() {
    folded(
        "fn one() -> u32 { 1 } fn two() -> u32 { 2 } fn main() { std::println!(\"{:?}\", (one(), two())); }",
        "fn two() -> u32 { 2 } fn one() -> u32 { 1 } fn main() { std::println!(\"{:?}\", (one(), two())); }",
    );
}

#[test]
fn qualified_built_in_print_macro_freezes_when_type_shadowed() {
    distinct(
        "struct std; fn one() -> u32 { 1 } fn two() -> u32 { 2 } fn main() { std::println!(\"{:?}\", (one(), two())); }",
        "struct std; fn two() -> u32 { 2 } fn one() -> u32 { 1 } fn main() { std::println!(\"{:?}\", (one(), two())); }",
    );
    distinct(
        "struct m; fn one() -> u32 { 1 } fn two() -> u32 { 2 } fn main() { m::println!(\"{:?}\", (one(), two())); }",
        "struct m; fn two() -> u32 { 2 } fn one() -> u32 { 1 } fn main() { m::println!(\"{:?}\", (one(), two())); }",
    );
}

#[test]
fn unproven_print_macro_root_keeps_item_groups_atomic() {
    distinct(
        "fn one() -> u32 { 1 } fn two() -> u32 { 2 } fn main() { observer::println!(\"{:?}\", (one(), two())); }",
        "fn two() -> u32 { 2 } fn one() -> u32 { 1 } fn main() { observer::println!(\"{:?}\", (one(), two())); }",
    );
}

#[test]
fn crate_root_paths_fold_when_unshadowed() {
    folded(
        "fn one() -> usize { let n = 1u32; std::mem::size_of_val(&n) } fn two() -> u32 { 2 } fn main() { println!(\"{:?}\", (one(), two())); }",
        "fn two() -> u32 { 2 } fn one() -> usize { let n = 1u32; std::mem::size_of_val(&n) } fn main() { println!(\"{:?}\", (one(), two())); }",
    );
}

#[test]
fn crate_root_paths_freeze_when_import_shadowed() {
    distinct(
        "use foo::std; fn one() -> usize { let n = 1u32; std::mem::size_of_val(&n) } fn two() -> u32 { 2 } fn main() { println!(\"{:?}\", (one(), two())); }",
        "use foo::std; fn two() -> u32 { 2 } fn one() -> usize { let n = 1u32; std::mem::size_of_val(&n) } fn main() { println!(\"{:?}\", (one(), two())); }",
    );
}

#[test]
fn prelude_names_fold_only_through_imports() {
    distinct(
        "fn one() -> Vec<u32> { Vec::new() } fn two() -> u32 { 2 } fn main() { println!(\"{:?}\", (one(), two())); }",
        "fn two() -> u32 { 2 } fn one() -> Vec<u32> { Vec::new() } fn main() { println!(\"{:?}\", (one(), two())); }",
    );
    folded(
        "use alloc::vec::Vec; fn one() -> Vec<u32> { Vec::new() } fn two() -> u32 { 2 } fn main() { println!(\"{:?}\", (one(), two())); }",
        "use alloc::vec::Vec; fn two() -> u32 { 2 } fn one() -> Vec<u32> { Vec::new() } fn main() { println!(\"{:?}\", (one(), two())); }",
    );
}

#[test]
fn crate_root_owner_spellings_fold_under_module_shadow() {
    folded(
        "struct S; mod m { pub struct S; } impl crate::S { fn one() -> u32 { 1 } } impl crate::S { fn two() -> u32 { 2 } }",
        "struct S; mod m { pub struct S; } impl S { fn one() -> u32 { 1 } } impl S { fn two() -> u32 { 2 } }",
    );
}

#[test]
fn crate_root_owner_keeps_root_and_module_terminals_separate() {
    distinct(
        "struct S; mod m { pub struct S; } impl crate::S { fn f() -> u32 { Self::g() } } impl crate::S { fn g() -> u32 { 1 } }",
        "struct S; mod m { pub struct S; } impl crate::m::S { fn f() -> u32 { Self::g() } } impl crate::m::S { fn g() -> u32 { 1 } }",
    );
}

#[test]
fn crate_root_module_owner_spellings_fold_under_shadow() {
    folded(
        "struct S; mod m { pub struct S; } impl crate::m::S { fn one() -> u32 { 1 } } impl crate::m::S { fn two() -> u32 { 2 } }",
        "struct S; mod m { pub struct S; } impl m::S { fn one() -> u32 { 1 } } impl m::S { fn two() -> u32 { 2 } }",
    );
}

#[test]
fn self_value_paths_fold_under_local_and_function_shadow() {
    folded(
        "mod m { pub fn one() -> u32 { 1 } } fn m() -> u32 { 0 } fn two() -> u32 { 2 } fn main() { let m = 9; self::m::one(); self::m(); two(); println!(\"{:?}\", (m, self::m::one(), self::m(), two())); }",
        "mod m { pub fn one() -> u32 { 1 } } fn two() -> u32 { 2 } fn m() -> u32 { 0 } fn main() { let m = 9; self::m::one(); self::m(); two(); println!(\"{:?}\", (m, self::m::one(), self::m(), two())); }",
    );
}

#[test]
fn self_value_path_folds_with_bare_spelling() {
    folded(
        "mod m { pub fn g() -> u32 { 1 } pub fn f() -> u32 { self::g() } } fn main() { println!(\"{}\", m::f()); }",
        "mod m { pub fn g() -> u32 { 1 } pub fn f() -> u32 { g() } } fn main() { println!(\"{}\", m::f()); }",
    );
}

#[test]
fn nested_trait_self_alpha_fold_inside_inherent_method() {
    let left = "struct S; impl S { fn outer() -> u32 { trait T { fn g() -> u32 { 1 } fn f(&self) -> u32 { Self::g() } } 0 } }";
    let right = "struct S; impl S { fn outer() -> u32 { trait U { fn g() -> u32 { 1 } fn f(&self) -> u32 { Self::g() } } 0 } }";
    assert_eq!(form(left), form(right), "{left}\n{right}");
    let renamed_members = "struct S; impl S { fn outer() -> u32 { trait U { fn h() -> u32 { 1 } fn e(&self) -> u32 { Self::h() } } 0 } }";
    assert_ne!(form(left), form(renamed_members));
}

#[test]
fn nested_trait_self_keeps_outer_target_separate() {
    distinct(
        "struct S; impl S { fn g() -> u32 { 9 } fn outer() -> u32 { trait T { fn g() -> u32 { 1 } fn f(&self) -> u32 { Self::g() } } 0 } }",
        "struct S; impl S { fn g() -> u32 { 9 } fn outer() -> u32 { trait T { fn g() -> u32 { 1 } fn f(&self) -> u32 { S::g() } } 0 } }",
    );
}

#[test]
fn crate_root_owner_does_not_merge_with_a_lexically_shadowing_owner() {
    distinct(
        "struct S; mod m { pub struct S; impl crate::S { fn one() -> u32 { 1 } } impl S { fn two() -> u32 { 2 } } }",
        "struct S; mod m { pub struct S; impl S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } } }",
    );
}

fn method_form(
    context: &syn_canon::SourceContext<'_>,
    source: &syn::File,
    name: &str,
) -> syn_canon::CanonicalForm {
    source
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Impl(impl_item) => impl_item.items.iter().find_map(|member| match member {
                syn::ImplItem::Fn(function) if function.sig.ident == name => Some(
                    context
                        .function(&function.sig, &function.block)
                        .unwrap()
                        .canonicalize(),
                ),
                _ => None,
            }),
            _ => None,
        })
        .unwrap()
}

#[test]
fn borrowed_views_match_the_standalone_qualified_partition_fold() {
    let split: syn::File = syn::parse_file(
        "mod m { pub struct S; } impl m::S { fn one() -> u32 { 1 } } impl m::S { fn two() -> u32 { 2 } }",
    )
    .unwrap();
    let combined: syn::File = syn::parse_file(
        "mod m { pub struct S; } impl m::S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } }",
    )
    .unwrap();
    let split_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &split)));
    let combined_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &combined)));
    assert_eq!(
        method_form(&split_context, &split, "one"),
        method_form(&combined_context, &combined, "one"),
    );
}

#[test]
fn borrowed_views_fold_under_module_shadow() {
    let split: syn::File = syn::parse_file(
        "struct S; mod m { pub struct S; } impl m::S { fn one() -> u32 { let _s = m::S; 1 } } impl m::S { fn two() -> u32 { 2 } }",
    )
    .unwrap();
    let combined: syn::File = syn::parse_file(
        "struct S; mod m { pub struct S; } impl m::S { fn one() -> u32 { let _s = m::S; 1 } fn two() -> u32 { 2 } }",
    )
    .unwrap();
    let split_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &split)));
    let combined_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &combined)));
    assert_eq!(
        method_form(&split_context, &split, "one"),
        method_form(&combined_context, &combined, "one"),
    );
}

#[test]
fn borrowed_views_keep_module_provenance_separate() {
    let root_ref: syn::File = syn::parse_file(
        "struct S; mod m { pub struct S; } impl S { fn f() -> u32 { let _s = S; 1 } }",
    )
    .unwrap();
    let module_ref: syn::File = syn::parse_file(
        "struct S; mod m { pub struct S; } impl S { fn f() -> u32 { let _s = m::S; 1 } }",
    )
    .unwrap();
    let root_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &root_ref)));
    let module_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &module_ref)));
    assert_ne!(
        method_form(&root_context, &root_ref, "f"),
        method_form(&module_context, &module_ref, "f"),
    );
}

#[test]
fn borrowed_views_keep_crate_owner_spelling_distinct() {
    let crate_spelling: syn::File = syn::parse_file(
        "mod m { pub struct S; } impl crate::m::S { fn one() -> u32 { Self::two() } fn two() -> u32 { 2 } }",
    )
    .unwrap();
    let bare_spelling: syn::File = syn::parse_file(
        "mod m { pub struct S; } impl m::S { fn one() -> u32 { Self::two() } fn two() -> u32 { 2 } }",
    )
    .unwrap();
    let crate_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &crate_spelling)));
    let bare_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &bare_spelling)));
    assert_ne!(
        method_form(&crate_context, &crate_spelling, "one"),
        method_form(&bare_context, &bare_spelling, "one"),
    );
}

#[test]
fn crate_root_owner_partition_folds_in_a_closed_module_context() {
    folded(
        "mod m { struct S; } struct S; impl crate::S { fn one() -> u32 { 1 } } impl crate::S { fn two() -> u32 { 2 } }",
        "mod m { struct S; } struct S; impl S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } }",
    );
}

#[test]
fn exported_owner_keeps_inherent_partitions_opaque() {
    distinct(
        "mod m { struct S; } pub struct S; impl crate::S { fn one() -> u32 { 1 } } impl crate::S { fn two() -> u32 { 2 } }",
        "mod m { struct S; } pub struct S; impl S { fn one() -> u32 { 1 } fn two() -> u32 { 2 } }",
    );
}
