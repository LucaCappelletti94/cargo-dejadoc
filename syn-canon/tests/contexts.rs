//! Lexical context canonicalization fixtures.

use syn_canon::{Context, ContextKind, ContextWork, contexts};

/// One emitted candidate.
struct Site {
    kind: ContextKind,
    item: String,
    form: String,
}

/// All context candidates of `src`, in emission order, plus the measured work.
fn sites(src: &str) -> (Vec<Site>, ContextWork) {
    let file = syn::parse_str(src).unwrap_or_else(|e| panic!("{src}: {e}"));
    let mut sites = Vec::new();
    let work = contexts(&file, &mut |ctx: Context<'_>, tokens| {
        sites.push(Site {
            kind: ctx.kind,
            item: ctx.item.to_string(),
            form: tokens.to_string(),
        });
    });
    (sites, work)
}

/// The canonical forms of every candidate of `kind`.
fn forms_of(sites: &[Site], kind: ContextKind) -> Vec<&str> {
    sites
        .iter()
        .filter(|site| site.kind == kind)
        .map(|site| site.form.as_str())
        .collect()
}

#[test]
fn a_renamed_parameter_body_is_one_form() {
    let (a, _) = sites("fn f(x: i32) -> i32 { x + x }");
    let (b, _) = sites("fn g(y: i32) -> i32 { y + y }");
    let fa = forms_of(&a, ContextKind::FunctionBody);
    let fb = forms_of(&b, ContextKind::FunctionBody);
    assert_eq!(fa.len(), 1);
    assert_eq!(fb.len(), 1);
    assert_eq!(fa[0], fb[0]);
}

#[test]
fn renamed_locals_and_captures_merge() {
    let (a, _) = sites("fn a(x: i32) -> i32 { let t = x + 1; t + t }");
    let (b, _) = sites("fn b(y: i32) -> i32 { let s = y + 1; s + s }");
    let fa = forms_of(&a, ContextKind::FunctionBody);
    let fb = forms_of(&b, ContextKind::FunctionBody);
    assert_eq!(fa[0], fb[0]);
}

#[test]
fn repeated_and_distinct_captures_stay_apart() {
    let (a, _) = sites("fn a(x: i32, y: i32) -> i32 { x + x }");
    let (b, _) = sites("fn b(x: i32, y: i32) -> i32 { x + y }");
    let fa = forms_of(&a, ContextKind::FunctionBody);
    let fb = forms_of(&b, ContextKind::FunctionBody);
    assert_ne!(fa[0], fb[0]);
}

#[test]
fn unrelated_declarations_do_not_shift_numbering() {
    let (a, _) = sites("fn a(x: i32) -> i32 { let unused_a = 99; { let t = x + 1; t + t } }");
    let (b, _) = sites("fn b(x: i32) -> i32 { { let t = x + 1; t + t } }");
    let fa = forms_of(&a, ContextKind::Block);
    let fb = forms_of(&b, ContextKind::Block);
    assert_eq!(fa.len(), 1);
    assert_eq!(fb.len(), 1);
    assert_eq!(fa[0], fb[0]);
}

#[test]
fn shadowing_resolves_to_nearest_binder() {
    let (a, _) = sites("fn a(x: i32) -> i32 { let x = x + 1; x + x }");
    let (b, _) = sites("fn b(y: i32) -> i32 { let y = y + 1; y + y }");
    assert_eq!(
        forms_of(&a, ContextKind::FunctionBody)[0],
        forms_of(&b, ContextKind::FunctionBody)[0]
    );

    // An inner shadow and an outer capture are different relationships.
    let (c, _) = sites("fn c(x: i32) -> i32 { { let x = x + 1; x + x } }");
    let (d, _) = sites("fn d(x: i32) -> i32 { { let y = x + 1; x + y } }");
    assert_ne!(
        forms_of(&c, ContextKind::Block)[0],
        forms_of(&d, ContextKind::Block)[0]
    );
}

#[test]
fn namespaces_keep_same_spelling_apart() {
    // The same shape under renamed type and value binders merges.
    let (a, _) = sites("type S = u8;\nfn a(s: S) -> u8 { { let x: S = s; x } }");
    let (b, _) = sites("type T = u8;\nfn b(t: T) -> u8 { { let y: T = t; y } }");
    assert_eq!(
        forms_of(&a, ContextKind::Block)[0],
        forms_of(&b, ContextKind::Block)[0]
    );

    // The same spelling in a type position and a value position must not merge.
    let (c, _) = sites("type x = u8;\nfn c() -> x { { let v: x = 1; v + v } }");
    let (d, _) = sites("fn d(x: i32) -> i32 { { let v: i32 = x; v + v } }");
    assert_ne!(
        forms_of(&c, ContextKind::Block)[0],
        forms_of(&d, ContextKind::Block)[0]
    );
}

#[test]
fn inherited_alias_targets_stay_associated() {
    let (a, _) = sites("use pinco as panco;\nfn a() -> i32 { panco::f() }");
    let (b, _) = sites("use pluto as panco;\nfn b() -> i32 { panco::f() }");
    assert_ne!(
        forms_of(&a, ContextKind::FunctionBody)[0],
        forms_of(&b, ContextKind::FunctionBody)[0]
    );

    // Renamed aliases to the same target merge.
    let (c, _) = sites("use pinco as panco;\nfn a() -> i32 { panco::f() }");
    let (d, _) = sites("use pinco as altro;\nfn b() -> i32 { altro::f() }");
    assert_eq!(
        forms_of(&c, ContextKind::FunctionBody)[0],
        forms_of(&d, ContextKind::FunctionBody)[0]
    );
}

#[test]
fn alias_chains_resolve_to_the_target() {
    let (a, _) = sites("use pinco as panco;\nfn a() -> i32 { panco::f() }");
    let (b, _) = sites("use pinco as primo;\nuse primo as panco;\nfn b() -> i32 { panco::f() }");
    assert_eq!(
        forms_of(&a, ContextKind::FunctionBody)[0],
        forms_of(&b, ContextKind::FunctionBody)[0]
    );
}

#[test]
fn complete_guarded_arms_merge() {
    let (a, _) = sites("fn a(x: i32) -> i32 { match x { y if y > 0 => y + 1, _ => 0 } }");
    let (b, _) = sites("fn b(z: i32) -> i32 { match z { w if w > 0 => w + 1, _ => 0 } }");
    let fa = forms_of(&a, ContextKind::Arm);
    let fb = forms_of(&b, ContextKind::Arm);
    assert_eq!(fa.len(), 2);
    assert_eq!(fb.len(), 2);
    assert_eq!(fa[0], fb[0]);
}

#[test]
fn unbraced_closures_merge_complete() {
    let (a, _) = sites("fn a(x: i32) -> i32 { let f = |p: i32| p + x; f(1) }");
    let (b, _) = sites("fn b(y: i32) -> i32 { let g = |q: i32| q + y; g(1) }");
    assert_eq!(
        forms_of(&a, ContextKind::Closure)[0],
        forms_of(&b, ContextKind::Closure)[0]
    );
}

#[test]
fn methods_and_trait_defaults_merge_across_owners() {
    let (a, _) = sites("struct A;\nimpl A { fn f(&self) -> i32 { self.mark() + self.tag() } }");
    let (b, _) = sites("struct B;\nimpl B { fn g(&self) -> i32 { self.mark() + self.tag() } }");
    assert_eq!(
        forms_of(&a, ContextKind::FunctionBody)[0],
        forms_of(&b, ContextKind::FunctionBody)[0]
    );

    let (c, _) = sites("trait T1 { fn d(&self) -> i32 { self.h() } }");
    let (d, _) = sites("trait T2 { fn e(&self) -> i32 { self.h() } }");
    assert_eq!(
        forms_of(&c, ContextKind::FunctionBody)[0],
        forms_of(&d, ContextKind::FunctionBody)[0]
    );
}

#[test]
fn nested_function_bodies_merge_and_report_the_item() {
    let (a, _) = sites("fn a() { fn inner(x: i32) -> i32 { x + x } inner(1) }");
    let (b, _) = sites("fn b() { fn other(y: i32) -> i32 { y + y } other(1) }");
    let fa: Vec<_> = a
        .iter()
        .filter(|site| site.item == "a::inner")
        .map(|site| site.form.as_str())
        .collect();
    let fb: Vec<_> = b
        .iter()
        .filter(|site| site.item == "b::other")
        .map(|site| site.form.as_str())
        .collect();
    assert_eq!(fa.len(), 1);
    assert_eq!(fb.len(), 1);
    assert_eq!(fa[0], fb[0]);
}

#[test]
fn unique_bodies_stay_unique() {
    let (a, _) = sites(
        "fn a(x: i32) -> i32 { x + 1 }\nfn b(x: i32) -> i32 { x - 1 }\nfn c(x: i32) -> i32 { x * 1 }",
    );
    let fa = forms_of(&a, ContextKind::FunctionBody);
    assert_eq!(fa.len(), 3);
    assert_ne!(fa[0], fa[1]);
    assert_ne!(fa[1], fa[2]);
    assert_ne!(fa[0], fa[2]);
}

#[test]
fn literals_fields_and_methods_stay_apart() {
    let (literal_one, _) = sites("fn a(x: i32) -> i32 { x + 1 }");
    let (literal_two, _) = sites("fn b(x: i32) -> i32 { x + 2 }");
    assert_ne!(
        forms_of(&literal_one, ContextKind::FunctionBody)[0],
        forms_of(&literal_two, ContextKind::FunctionBody)[0]
    );

    let (field_a, _) = sites("struct S;\nfn c(s: S) { s.a }");
    let (field_b, _) = sites("struct S;\nfn d(s: S) { s.b }");
    assert_ne!(
        forms_of(&field_a, ContextKind::FunctionBody)[0],
        forms_of(&field_b, ContextKind::FunctionBody)[0]
    );

    let (method_a, _) = sites("struct S;\nfn e(s: S) { s.a() }");
    let (method_b, _) = sites("struct S;\nfn f(s: S) { s.b() }");
    assert_ne!(
        forms_of(&method_a, ContextKind::FunctionBody)[0],
        forms_of(&method_b, ContextKind::FunctionBody)[0]
    );
}

#[test]
fn bound_order_is_preserved() {
    let left = "trait Left<U> {}\ntrait Right<U> {}\nfn a() { fn f<T, U>() where T: Left<U>, T: Right<U> {} f::<i32, i32>() }";
    let right = "trait Left<U> {}\ntrait Right<U> {}\nfn b() { fn f<T, U>() where T: Right<U>, T: Left<U> {} f::<i32, i32>() }";
    let (a, _) = sites(left);
    let (b, _) = sites(right);
    assert_ne!(
        forms_of(&a, ContextKind::FunctionBody)[0],
        forms_of(&b, ContextKind::FunctionBody)[0]
    );
}

#[test]
fn block_wide_imports_and_items() {
    // A use after its use-site is block-wide, like in the drifted pipeline.
    let (a, _) = sites("fn a() -> i32 { panco::f(); use pinco as panco; }");
    let (b, _) = sites("fn b() -> i32 { use pinco as panco; panco::f(); }");
    assert_eq!(
        forms_of(&a, ContextKind::FunctionBody)[0],
        forms_of(&b, ContextKind::FunctionBody)[0]
    );

    // A nested function is visible before its declaration.
    let (c, _) = sites("fn a() -> i32 { local(); fn local() -> i32 { 1 } }");
    let (d, _) = sites("fn b() -> i32 { fn local() -> i32 { 1 } local(); }");
    assert_eq!(
        forms_of(&c, ContextKind::FunctionBody)[0],
        forms_of(&d, ContextKind::FunctionBody)[0]
    );
}

#[test]
fn sequential_lets_see_nearest_origin() {
    // The block before the shadow sees the parameter, the block after sees the new local.
    let (a, _) = sites("fn a(x: i32) -> i32 { let x = x + 1; { x + x } }");
    let (b, _) = sites("fn b(x: i32) -> i32 { { x + x } let x = x + 1; }");
    assert_eq!(
        forms_of(&a, ContextKind::Block)[0],
        forms_of(&b, ContextKind::Block)[0]
    );
}

#[test]
fn inherited_block_imports_apply_to_nested_candidates() {
    let (a, _) = sites("fn a() { use pinco as alias; consume({ alias::f() }); }");
    let (b, _) = sites("fn b() { use pluto as alias; consume({ alias::f() }); }");
    assert_ne!(
        forms_of(&a, ContextKind::Block),
        forms_of(&b, ContextKind::Block)
    );
}

#[test]
fn braced_closures_and_arms_include_their_explicit_blocks() {
    let (found, _) = sites("fn a(x: u32) { let f = |y| { x + y }; match x { z => { z + 1 } }; }");
    assert_eq!(forms_of(&found, ContextKind::FunctionBody).len(), 1);
    assert_eq!(forms_of(&found, ContextKind::Closure).len(), 1);
    assert_eq!(forms_of(&found, ContextKind::Arm).len(), 1);
    assert_eq!(forms_of(&found, ContextKind::Block).len(), 2);
}

#[test]
fn grouped_inherited_imports_apply_to_nested_candidates() {
    let (a, _) = sites("fn a() { use pinco::{f as fa, g as fg}; consume({ fa() + fg() }); }");
    let (b, _) = sites("fn b() { use pluto::{f as fa, g as fg}; consume({ fa() + fg() }); }");
    assert_ne!(
        forms_of(&a, ContextKind::Block),
        forms_of(&b, ContextKind::Block)
    );

    // Renamed aliases of one grouped target still merge.
    let (c, _) = sites("fn a() { use pinco::{f as fa, g as fg}; consume({ fa() + fg() }); }");
    let (d, _) = sites("fn a() { use pinco::{f as ba, g as bg}; consume({ ba() + bg() }); }");
    assert_eq!(
        forms_of(&c, ContextKind::Block),
        forms_of(&d, ContextKind::Block)
    );
}

#[test]
fn forward_block_wide_alias_applies_to_earlier_blocks() {
    let (a, _) = sites("fn a() { consume({ panco::f() }); use pinco as panco; }");
    let (b, _) = sites("fn b() { consume({ panco::f() }); use pluto as panco; }");
    assert_ne!(
        forms_of(&a, ContextKind::Block),
        forms_of(&b, ContextKind::Block)
    );
}

#[test]
fn locally_declared_import_targets_stay_distinct() {
    let (a, _) = sites(
        "fn a() { mod pinco { pub fn f() {} } use pinco as panco; consume({ panco::f() }); }",
    );
    let (b, _) = sites(
        "fn b() { mod pluto { pub fn f() {} } use pluto as panco; consume({ panco::f() }); }",
    );
    assert_ne!(
        forms_of(&a, ContextKind::Block),
        forms_of(&b, ContextKind::Block)
    );

    // Renamed aliases of one locally declared target still merge.
    let (c, _) = sites(
        "fn a() { mod pinco { pub fn f() {} } use pinco as panco; consume({ panco::f() }); }",
    );
    let (d, _) = sites(
        "fn a() { mod pinco { pub fn f() {} } use pinco as autre; consume({ autre::f() }); }",
    );
    assert_eq!(
        forms_of(&c, ContextKind::Block),
        forms_of(&d, ContextKind::Block)
    );
}

#[test]
fn preceding_locals_merge_by_first_reference() {
    let (a, _) = sites("fn a() { let x = 1; consume(|| x + x); }");
    let (b, _) = sites("fn b() { let y = 1; consume(|| y + y); }");
    assert_eq!(
        forms_of(&a, ContextKind::Closure)[0],
        forms_of(&b, ContextKind::Closure)[0]
    );
}

#[test]
fn shadowed_preceding_local_keeps_one_origin() {
    let (a, _) = sites("fn a() { let x = 1; let x = x + 1; consume({ x + x }); }");
    let (b, _) = sites("fn b() { let y = 1; let y = y + 1; consume({ y + y }); }");
    assert_eq!(
        forms_of(&a, ContextKind::Block)[0],
        forms_of(&b, ContextKind::Block)[0]
    );
}

#[test]
fn repeated_and_distinct_preceding_locals_stay_apart() {
    let (a, _) = sites("fn a() { let x = 1; let y = 2; consume({ x + x }); }");
    let (b, _) = sites("fn b() { let p = 1; let q = 2; consume({ p + q }); }");
    assert_ne!(
        forms_of(&a, ContextKind::Block)[0],
        forms_of(&b, ContextKind::Block)[0]
    );
}

#[test]
fn const_generic_default_blocks_emit_once() {
    // A const generic default block is a source block the signature walk visits once.
    let (sites, _) = sites(
        "struct S<const N: usize = { 3 }>; fn a<const M: usize = { 3 }>() { consume({ 1 }); }",
    );
    assert_eq!(forms_of(&sites, ContextKind::Block).len(), 3);
}

#[test]
fn capture_numbering_depends_on_references_not_parameter_order() {
    let (a, _) = sites("fn a(x: u32, y: u32) { consume({ x + y + x }); }");
    let (b, _) = sites("fn b(y: u32, x: u32) { consume({ x + y + x }); }");
    assert_eq!(
        forms_of(&a, ContextKind::Block),
        forms_of(&b, ContextKind::Block)
    );
}

#[test]
fn literal_values_that_resemble_canonical_names_are_preserved() {
    struct Strings(Vec<String>);
    impl<'ast> syn::visit::Visit<'ast> for Strings {
        fn visit_lit_str(&mut self, literal: &'ast syn::LitStr) {
            self.0.push(literal.value());
        }
    }
    let (found, _) = sites("fn a(x: u32, y: u32) { consume({ (y, x, \"_iv0\") }); }");
    let block = syn::parse_str(forms_of(&found, ContextKind::Block)[0]).unwrap();
    let mut strings = Strings(Vec::new());
    syn::visit::Visit::visit_block(&mut strings, &block);
    assert_eq!(strings.0, ["_iv0"]);
}

#[test]
fn const_capture_identity_is_shared_across_type_and_value_positions() {
    let (a, _) = sites("fn a<T, const N: usize>() { consume({ pair::<T, N>(); value(N); }); }");
    let (b, _) = sites(
        "fn b<T, const N: usize, const M: usize>() { consume({ pair::<T, N>(); value(M); }); }",
    );
    assert_ne!(
        forms_of(&a, ContextKind::Block),
        forms_of(&b, ContextKind::Block)
    );
}

#[test]
fn inherited_import_paths_preserve_generic_literals() {
    let (a, _) = sites("use ext::make as Alias; fn a() { consume({ Alias::<1>(); }); }");
    let (b, _) = sites("use ext::make as Alias; fn b() { consume({ Alias::<2>(); }); }");
    assert_ne!(
        forms_of(&a, ContextKind::Block),
        forms_of(&b, ContextKind::Block)
    );
}

#[test]
fn inherited_import_generic_captures_merge_after_renaming() {
    let (a, _) = sites("use ext::make as Alias; fn a<T>() { consume({ Alias::<T>(); }); }");
    let (b, _) = sites("use ext::make as Other; fn b<U>() { consume({ Other::<U>(); }); }");
    let (c, _) = sites("use ext::make as Alias; fn c<T, U>() { consume({ Alias::<(T, U)>(); }); }");
    let a = forms_of(&a, ContextKind::Block);
    assert_eq!(a, forms_of(&b, ContextKind::Block));
    assert_ne!(a, forms_of(&c, ContextKind::Block));
}

#[test]
fn qualified_import_targets_ignore_unrelated_leaf_aliases() {
    let (north, _) = sites(
        "use unused::Item as Item; use north::Item as Alias; fn a() { consume({ Alias::new() }); }",
    );
    let (south, _) = sites(
        "use unused::Item as Item; use south::Item as Alias; fn b() { consume({ Alias::new() }); }",
    );
    let (plain, _) = sites("use north::Item as Alias; fn c() { consume({ Alias::new() }); }");
    let north = forms_of(&north, ContextKind::Block);
    assert_ne!(north, forms_of(&south, ContextKind::Block));
    assert_eq!(north, forms_of(&plain, ContextKind::Block));
}

#[test]
fn context_raw_import_targets_preserve_alias_associations() {
    let (first, _) = sites("use std::r#mem::r#drop as call; fn first() { call(String::new()); }");
    let (renamed, _) =
        sites("use std::r#mem::r#drop as invoke; fn second() { invoke(String::new()); }");
    let (different, _) =
        sites("use std::r#mem::r#forget as call; fn third() { call(String::new()); }");
    let first = forms_of(&first, ContextKind::FunctionBody);
    assert_eq!(first, forms_of(&renamed, ContextKind::FunctionBody));
    assert_ne!(first, forms_of(&different, ContextKind::FunctionBody));
}

#[test]
fn context_candidate_raw_import_targets_preserve_alias_associations() {
    let (first, _) = sites("fn first() { use std::r#mem::r#drop as call; call(String::new()); }");
    let (renamed, _) =
        sites("fn second() { use std::r#mem::r#drop as invoke; invoke(String::new()); }");
    let (different, _) =
        sites("fn third() { use std::r#mem::r#forget as call; call(String::new()); }");
    let first = forms_of(&first, ContextKind::FunctionBody);
    assert_eq!(first, forms_of(&renamed, ContextKind::FunctionBody));
    assert_ne!(first, forms_of(&different, ContextKind::FunctionBody));
}

#[test]
fn context_absolute_roots_distinguish_inherited_module_shadowing() {
    let (found, _) = sites(
        "mod std { pub mod mem { pub fn drop(_: String) {} } }
         fn first(value: String) { ::std::mem::drop(value); }
         fn renamed(other: String) { ::std::mem::drop(other); }
         fn local(value: String) { std::mem::drop(value); }",
    );
    let body = |item| {
        &found
            .iter()
            .find(|site| site.kind == ContextKind::FunctionBody && site.item == item)
            .unwrap()
            .form
    };
    assert_eq!(body("first"), body("renamed"));
    assert_ne!(body("first"), body("local"));
}

#[test]
fn context_unshadowed_absolute_roots_merge_with_relative_paths() {
    let (first, _) = sites("fn first(value: String) { ::std::mem::drop(value); }");
    let (second, _) = sites("fn second(other: String) { std::mem::drop(other); }");
    assert_eq!(
        forms_of(&first, ContextKind::FunctionBody),
        forms_of(&second, ContextKind::FunctionBody)
    );
}
