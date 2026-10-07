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

fn body_of<'a>(found: &'a [Site], item: &str) -> &'a str {
    &found
        .iter()
        .find(|site| site.kind == ContextKind::FunctionBody && site.item == item)
        .unwrap()
        .form
}

#[test]
fn imported_absolute_roots_preserve_file_scope_shadowing() {
    for imports in [
        "use ::std::mem::drop as external; use std::mem::drop as local;",
        "use ::{std::mem::drop as external}; use std::mem::drop as local;",
    ] {
        let source = format!(
            "mod std {{ pub mod mem {{ pub fn drop(_: String) {{}} }} }}
             {imports}
             fn first(value: String) {{ external(value); }}
             fn second(other: String) {{ local(other); }}"
        );
        let (found, _) = sites(&source);
        assert_ne!(body_of(&found, "first"), body_of(&found, "second"));
    }
}

#[test]
fn imported_absolute_roots_preserve_block_scope_shadowing() {
    let (found, _) = sites(
        "fn first(value: String, other: String) {
             mod std { pub mod mem { pub fn drop(_: String) {} } }
             use ::std::mem::drop as external;
             use std::mem::drop as local;
             { external(value); }
             { local(other); }
         }",
    );
    let blocks = forms_of(&found, ContextKind::Block);
    assert_eq!(blocks.len(), 2);
    assert_ne!(blocks[0], blocks[1]);
}

#[test]
fn imported_absolute_roots_bypass_lexical_aliases() {
    let (found, _) = sites(
        "mod local_module { pub mod mem { pub fn drop(_: String) {} } }
         use local_module as std;
         use ::std::mem::drop as external;
         use std::mem::drop as local;
         fn first(value: String) { external(value); }
         fn second(other: String) { local(other); }",
    );
    assert_ne!(body_of(&found, "first"), body_of(&found, "second"));
}

#[test]
fn imported_unshadowed_absolute_and_relative_roots_merge() {
    for imports in [
        "use ::std::mem::drop as external; use std::mem::drop as local;",
        "use ::{std::mem::drop as external}; use std::mem::drop as local;",
    ] {
        let source = format!(
            "{imports}
             fn first(value: String) {{ external(value); }}
             fn second(other: String) {{ local(other); }}"
        );
        let (found, _) = sites(&source);
        assert_eq!(body_of(&found, "first"), body_of(&found, "second"));
    }
}

#[test]
fn required_trait_signatures_discover_blocks_with_method_paths() {
    let (found, _) = sites(
        "pub trait T {
             fn first<'a>(_: &'a [u8; { 1 + 1 }]);
             fn second<'b>(_: &'b [u8; { 1 + 1 }]);
         }",
    );
    assert_eq!(found.len(), 2);
    for (site, item) in found.iter().zip(["T::first", "T::second"]) {
        assert_eq!(site.kind, ContextKind::Block);
        assert_eq!(site.item, item);
    }
    assert_eq!(found[0].form, found[1].form);
}

#[test]
fn default_trait_signatures_keep_bodies_and_signature_blocks() {
    let (found, _) = sites(
        "pub trait T {
             fn first(_: [u8; { 1 + 1 }]) {}
             fn second(_: [u8; { 1 + 1 }]) {}
         }",
    );
    assert_eq!(found.len(), 4);
    assert_eq!(forms_of(&found, ContextKind::Block).len(), 2);
    assert_eq!(forms_of(&found, ContextKind::FunctionBody).len(), 2);
    assert!(
        found
            .iter()
            .all(|site| ["T::first", "T::second"].contains(&site.item.as_str()))
    );
}

#[test]
fn foreign_signatures_discover_blocks_with_function_paths() {
    let (found, _) = sites(
        "unsafe extern \"C\" {
             fn first(_: [u8; { 1 + 1 }]);
             fn second(_: [u8; { 1 + 1 }]);
         }",
    );
    assert_eq!(found.len(), 2);
    for (site, item) in found.iter().zip(["first", "second"]) {
        assert_eq!(site.kind, ContextKind::Block);
        assert_eq!(site.item, item);
    }
    assert_eq!(found[0].form, found[1].form);
}

#[test]
fn contextual_self_imports_preserve_module_targets() {
    let (found, _) = sites(
        "use std::mem::{self};
         fn first(value: String) { mem::drop(value); }
         fn second(other: String) { std::mem::drop(other); }",
    );
    assert_eq!(body_of(&found, "first"), body_of(&found, "second"));
}

#[test]
fn whole_file_self_imports_preserve_module_member_references() {
    for canonicalize in [syn_canon::canonicalize, syn_canon::canonicalize_failing] {
        let input = syn::parse_str(
            "mod local { pub fn f() {} }
             mod caller {
                 use super::local::{self};
                 pub fn g() { local::f(); }
             }",
        )
        .unwrap();
        let file: syn::File = syn::parse2(canonicalize(input)).unwrap();
        let modules: Vec<_> = file
            .items
            .iter()
            .filter_map(|item| match item {
                syn::Item::Mod(module) => module.content.as_ref().map(|(_, items)| items),
                _ => None,
            })
            .collect();
        let member = modules[0]
            .iter()
            .find_map(|item| match item {
                syn::Item::Fn(function) => Some(&function.sig.ident),
                _ => None,
            })
            .unwrap();
        let caller = modules[1]
            .iter()
            .find_map(|item| match item {
                syn::Item::Fn(function) => Some(&function.block),
                _ => None,
            })
            .unwrap();
        let syn::Stmt::Expr(syn::Expr::Call(call), _) = &caller.stmts[0] else {
            panic!("expected a module member call");
        };
        let syn::Expr::Path(path) = &*call.func else {
            panic!("expected a module path");
        };
        assert_eq!(path.path.segments.last().unwrap().ident, *member);
    }
}

#[test]
fn pattern_shapes_bind_captures_for_nested_blocks() {
    let (a, _) = sites(
        "struct Point { x: i32, y: i32 }
         struct Wrap(i32);
         fn first(cell: &i32) -> i32 {
             let (a, b) = (1, 2);
             let Point { x, y } = Point { x: a, y: b };
             let (c | c) = a;
             let &e = cell;
             let f: i32 = e + x + y + c;
             let h @ Wrap(w) = Wrap(f);
             { a + b + c + e + f + h.0 + w + x };
             { a + b }
         }",
    );
    let (b, _) = sites(
        "struct Point { x: i32, y: i32 }
         struct Wrap(i32);
         fn second(cell: &i32) -> i32 {
             let (p, q) = (1, 2);
             let Point { x: r, y: s } = Point { x: p, y: q };
             let (t | t) = p;
             let &v = cell;
             let m: i32 = v + r + s + t;
             let n @ Wrap(o) = Wrap(m);
             { p + q + t + v + m + n.0 + o + r };
             { p + q }
         }",
    );
    assert_eq!(body_of(&a, "first"), body_of(&b, "second"));
    let blocks_a = forms_of(&a, ContextKind::Block);
    let blocks_b = forms_of(&b, ContextKind::Block);
    assert_eq!(blocks_a.len(), 2);
    assert_eq!(blocks_a, blocks_b);
    assert_ne!(blocks_a[0], blocks_a[1]);
}

#[test]
fn let_else_and_loop_conditions_scope_their_bindings() {
    let (a, _) = sites(
        "fn first(v: i32) -> i32 {
             let Some(x) = Some(v) else { panic!() };
             if let Some(y) = Some(v + 1) {
                 x + y
             }
             while let Some(w) = Some(v + 2) {
                 break w;
             }
             for (p, q) in [(1, 2), (3, 4)] {
                 p + q + x
             }
             0
         }",
    );
    let (b, _) = sites(
        "fn second(v: i32) -> i32 {
             let Some(m) = Some(v) else { panic!() };
             if let Some(n) = Some(v + 1) {
                 m + n
             }
             while let Some(t) = Some(v + 2) {
                 break t;
             }
             for (u, z) in [(1, 2), (3, 4)] {
                 u + z + m
             }
             0
         }",
    );
    assert_eq!(a.len(), 5);
    assert_eq!(forms_of(&a, ContextKind::FunctionBody).len(), 1);
    let blocks_a = forms_of(&a, ContextKind::Block);
    let blocks_b = forms_of(&b, ContextKind::Block);
    assert_eq!(blocks_a.len(), 4);
    assert_eq!(blocks_a, blocks_b);
    assert_ne!(blocks_a[0], blocks_a[1]);
    assert_ne!(blocks_a[1], blocks_a[3]);
    assert_eq!(body_of(&a, "first"), body_of(&b, "second"));
}

#[test]
fn special_block_expressions_emit_their_own_candidates() {
    let (a, _) = sites(
        "fn first(v: i32) -> i32 {
             let r = unsafe { v + 1 };
             let _ = async { v + 1 };
             let _ = try { v + 1 };
             let _ = const { v + 1 };
             'mark: { let _ = v + 5; }
             { let _ = v + 5; }
             r
         }",
    );
    let (b, _) = sites(
        "fn second(v: i32) -> i32 {
             let s = unsafe { v + 1 };
             let _ = async { v + 1 };
             let _ = try { v + 1 };
             let _ = const { v + 1 };
             'note: { let _ = v + 5; }
             { let _ = v + 5; }
             s
         }",
    );
    assert_eq!(a.len(), 7);
    let blocks_a = forms_of(&a, ContextKind::Block);
    let blocks_b = forms_of(&b, ContextKind::Block);
    assert_eq!(blocks_a.len(), 6);
    assert_eq!(blocks_a, blocks_b);
    assert_ne!(blocks_a[0], blocks_a[1]);
    assert_ne!(blocks_a[2], blocks_a[3]);
    assert_ne!(blocks_a[4], blocks_a[5]);
    assert_eq!(body_of(&a, "first"), body_of(&b, "second"));
}

#[test]
fn inline_module_imports_and_item_kinds_discover_const_blocks() {
    let (a, _) = sites(
        "mod alpha {
             pub mod std { pub mod mem { pub fn drop(_: i32) {} } }
             use ::{std::mem::drop as taken};
             pub struct Rows([u8; { 1 + 1 }]);
             pub enum Kind { Small([u8; { 1 + 1 }]) }
             pub union Mix { a: [u8; { 1 + 1 }] }
             pub type Alias = [u8; { 1 + 1 }];
             pub trait Aliased = Iterator<Item = [u8; { 1 + 1 }]>;
             pub const COUNT: i32 = { 1 + 1 };
             pub static SLOT: i32 = { 1 + 1 };
             pub fn run() { taken(1); }
         }
         mod beta {
             pub mod std { pub mod mem { pub fn drop(_: i32) {} } }
             use {std::mem::drop as taken};
             pub fn run() { taken(1); }
         }",
    );
    let items: Vec<&str> = a
        .iter()
        .filter(|site| site.kind == ContextKind::Block)
        .map(|site| site.item.as_str())
        .collect();
    assert_eq!(
        items,
        vec![
            "alpha::Rows",
            "alpha::Kind",
            "alpha::Mix",
            "alpha::Alias",
            "alpha::Aliased",
            "alpha::COUNT",
            "alpha::SLOT"
        ]
    );
    let blocks = forms_of(&a, ContextKind::Block);
    assert!(blocks.iter().all(|form| *form == blocks[0]));
    assert_ne!(body_of(&a, "alpha::run"), body_of(&a, "beta::run"));
}

#[test]
fn impl_and_trait_associated_items_discover_their_blocks() {
    let (a, _) = sites(
        "struct S;
         struct Box2<T>(T);
         impl S {
             const C: i32 = { 1 + 1 };
             type X = [u8; { 1 + 1 }];
             m!();
             fn f(&self) -> i32 { self.g() }
             fn g(&self) -> i32 { 1 }
         }
         impl<T: Clone> Box2<T> {
             fn take(&self) -> T { self.0.clone() }
         }
         impl Vec<u8> {
             fn h(&self) -> usize { self.len() }
         }
         trait T {
             const C: i32;
             type X = [u8; { 1 + 1 }];
             m!();
         }",
    );
    let items: Vec<&str> = a
        .iter()
        .filter(|site| site.kind == ContextKind::Block)
        .map(|site| site.item.as_str())
        .collect();
    assert_eq!(items, vec!["S::C", "S::X", "T::X"]);
    let blocks = forms_of(&a, ContextKind::Block);
    assert!(blocks.iter().all(|form| *form == blocks[0]));
    assert_eq!(a.len(), 7);
    assert!(
        a.iter()
            .any(|site| site.kind == ContextKind::FunctionBody && site.item == "Box2 < T >::take")
    );
    assert!(
        a.iter()
            .any(|site| site.kind == ContextKind::FunctionBody && site.item == "Vec < u8 >::h")
    );
    let (b, _) = sites(
        "struct U;
         struct Pair<P>(P);
         impl U {
             const D: i32 = { 1 + 1 };
             type Y = [u8; { 1 + 1 }];
             n!();
             fn f(&self) -> i32 { self.g() }
             fn g(&self) -> i32 { 1 }
         }
         impl<P: Clone> Pair<P> {
             fn grab(&self) -> P { self.0.clone() }
         }
         impl Option<u8> {
             fn t(&self) -> bool { self.is_some() }
         }
         trait V {
             const D: i32;
             type Y = [u8; { 1 + 1 }];
             n!();
         }",
    );
    assert_eq!(
        forms_of(&a, ContextKind::Block),
        forms_of(&b, ContextKind::Block)
    );
    assert_eq!(body_of(&a, "S::f"), body_of(&b, "U::f"));
    assert_eq!(body_of(&a, "S::g"), body_of(&b, "U::g"));
}

#[test]
fn higher_ranked_binds_and_fn_pointers_frame_their_lifetimes() {
    let (a, _) = sites(
        "fn first<'a>(x: &'a i32, g: impl for<'b> Fn(&'b i32) -> i32) -> i32 {
             let f: for<'a> fn(&'a i32) -> &'a i32;
             let h = for<'c> |y: &'c i32| y + 1;
             { g(x) + h(&x) + 0 }
         }",
    );
    let (b, _) = sites(
        "fn second<'a>(x: &'a i32, k: impl for<'b> Fn(&'b i32) -> i32) -> i32 {
             let q: for<'a> fn(&'a i32) -> &'a i32;
             let j = for<'c> |y: &'c i32| y + 1;
             { k(x) + j(&x) + 0 }
         }",
    );
    assert_eq!(a.len(), 3);
    assert_eq!(body_of(&a, "first"), body_of(&b, "second"));
    assert_eq!(
        forms_of(&a, ContextKind::Block),
        forms_of(&b, ContextKind::Block)
    );
    assert_eq!(
        forms_of(&a, ContextKind::Closure),
        forms_of(&b, ContextKind::Closure)
    );
}

#[test]
fn plain_and_glob_imports_resolve_nested_candidates() {
    let (a, _) = sites(
        "mod plain {
             pub fn f() -> i32 { 1 }
             pub mod sub {
                 pub fn g() -> i32 { 2 }
             }
         }
         use plain::f;
         use plain::sub::*;
         fn first() -> i32 {
             { f() + g() }
         }",
    );
    let (b, _) = sites(
        "mod simple {
             pub fn k() -> i32 { 1 }
             pub mod extra {
                 pub fn j() -> i32 { 2 }
             }
         }
         use simple::k;
         use simple::extra::*;
         fn second() -> i32 {
             { k() + j() }
         }",
    );
    assert_eq!(a.len(), 4);
    assert_ne!(body_of(&a, "first"), body_of(&b, "second"));
    assert_ne!(
        forms_of(&a, ContextKind::Block),
        forms_of(&b, ContextKind::Block)
    );
    let (same_targets, _) = sites(
        "use plain::f;
         use plain::sub::*;
         fn renamed() -> i32 { { f() + g() } }",
    );
    assert_eq!(body_of(&a, "first"), body_of(&same_targets, "renamed"));
}

#[test]
fn opaque_macro_inputs_keep_unresolved_spellings() {
    let (a, _) = sites("fn a(x: i32) -> i32 { emit!(w + x); x + 1 }");
    let (b, _) = sites("fn b(y: i32) -> i32 { emit!(w + y); y + 1 }");
    assert_eq!(body_of(&a, "a"), body_of(&b, "b"));
    let (c, _) = sites("fn c(x: i32) -> i32 { emit!(u + x); x + 1 }");
    assert_ne!(body_of(&a, "a"), body_of(&c, "c"));
}

#[test]
fn work_reports_candidate_and_token_totals() {
    let (found, work) = sites("fn a(x: i32) -> i32 { x + 1 }");
    assert_eq!(work.contexts, found.len());
    assert_eq!(work.input_tokens, 3);
    let (_, nested_work) = sites("fn b(x: i32) -> i32 { { x + 1 } }");
    assert_eq!(nested_work.contexts, 2);
    assert_eq!(nested_work.input_tokens, 6);
    let (_, blank_work) = sites("fn empty() {}");
    assert_eq!(blank_work.contexts, 1);
    assert_eq!(blank_work.input_tokens, 0);
}

#[test]
fn labeled_loop_scopes_preserve_break_targets() {
    let (a, _) = sites(
        "fn first() {
             'again: for value in [1] { if value > 0 { continue 'again; } }
             'repeat: while ready() { break 'repeat; }
             'forever: loop { break 'forever; }
         }",
    );
    let (b, _) = sites(
        "fn second() {
             'next: for item in [1] { if item > 0 { continue 'next; } }
             'retry: while ready() { break 'retry; }
             'stop: loop { break 'stop; }
         }",
    );
    assert_eq!(body_of(&a, "first"), body_of(&b, "second"));
    assert_eq!(
        forms_of(&a, ContextKind::Block),
        forms_of(&b, ContextKind::Block)
    );
    assert_eq!(forms_of(&a, ContextKind::Block).len(), 4);
    let (different, _) = sites(
        "fn third() {
             'next: for item in [1] { if item > 0 { break 'next; } }
             'retry: while ready() { break 'retry; }
             'stop: loop { break 'stop; }
         }",
    );
    assert_ne!(body_of(&a, "first"), body_of(&different, "third"));
}

#[test]
fn scoped_macro_declarations_bind_nested_references() {
    let (a, _) = sites("fn first() { macro_rules! local { () => { 1 } } { local!() } }");
    let (b, _) = sites("fn second() { macro_rules! renamed { () => { 1 } } { renamed!() } }");
    assert_eq!(body_of(&a, "first"), body_of(&b, "second"));
    assert_eq!(
        forms_of(&a, ContextKind::Block),
        forms_of(&b, ContextKind::Block)
    );
    let (different, _) = sites("fn third() { { external!() } }");
    assert_ne!(
        forms_of(&a, ContextKind::Block),
        forms_of(&different, ContextKind::Block)
    );
}

#[test]
fn candidate_inline_self_imports_keep_the_written_module_target() {
    let (a, _) = sites(
        "fn first() {
             mod caller {
                 use std::mem::{self};
                 pub fn run(value: String) { mem::drop(value) }
             }
             caller::run(String::new());
         }",
    );
    let (b, _) = sites(
        "fn second() {
             mod caller {
                 use std::mem::{self};
                 pub fn run(other: String) { mem::drop(other) }
             }
             caller::run(String::new());
         }",
    );
    assert_eq!(body_of(&a, "first"), body_of(&b, "second"));
    let (different, _) = sites(
        "fn third() {
             mod caller {
                 use other::mem::{self};
                 pub fn run(value: String) { mem::drop(value) }
             }
             caller::run(String::new());
         }",
    );
    assert_ne!(body_of(&a, "first"), body_of(&different, "third"));
}

#[test]
fn independent_arms_normalize_literal_and_parenthesis_spelling() {
    let (a, _) = sites(
        "fn first(value: Option<u32>) -> u32 {
             match value { Some(x) => ((0x10_u32 + x)), None => 0 }
         }",
    );
    let (b, _) = sites(
        "fn second(value: Option<u32>) -> u32 {
             match value { Some(y) => 16_u32 + y, None => 0 }
         }",
    );
    assert_eq!(
        forms_of(&a, ContextKind::Arm),
        forms_of(&b, ContextKind::Arm)
    );
    let (different, _) = sites(
        "fn third(value: Option<u32>) -> u32 {
             match value { Some(x) => 17_u32 + x, None => 0 }
         }",
    );
    assert_ne!(
        forms_of(&a, ContextKind::Arm),
        forms_of(&different, ContextKind::Arm)
    );
}

#[test]
fn independent_closures_normalize_literal_and_parenthesis_spelling() {
    let (a, _) = sites("fn first() { let f = |x: u32| ((0x10_u32 + x)); consume(f); }");
    let (b, _) = sites("fn second() { let g = |y: u32| 16_u32 + y; consume(g); }");
    assert_eq!(
        forms_of(&a, ContextKind::Closure),
        forms_of(&b, ContextKind::Closure)
    );
    let (different, _) = sites("fn third() { let h = |x: u32| 17_u32 + x; consume(h); }");
    assert_ne!(
        forms_of(&a, ContextKind::Closure),
        forms_of(&different, ContextKind::Closure)
    );
}

#[test]
fn independent_function_bodies_fold_their_own_tail_return() {
    let (a, _) = sites("fn first(x: u32) -> u32 { return x + x; }");
    let (b, _) = sites("fn second(y: u32) -> u32 { y + y }");
    assert_eq!(body_of(&a, "first"), body_of(&b, "second"));
    let (different, _) = sites("fn third(x: u32) -> u32 { if ready() { return x + x; } x - x }");
    assert_ne!(body_of(&a, "first"), body_of(&different, "third"));
}

#[test]
fn imported_values_do_not_collapse_trait_and_generic_origins() {
    let (found, _) = sites(
        "use std::mem::drop as TraitName;
         trait TraitName<T>: Sized {
             fn distinct() -> usize {
                 std::mem::size_of::<Self>() + std::mem::size_of::<T>()
             }
             fn repeated() -> usize {
                 std::mem::size_of::<Self>() + std::mem::size_of::<Self>()
             }
         }",
    );
    assert_ne!(
        body_of(&found, "TraitName::distinct"),
        body_of(&found, "TraitName::repeated")
    );
}

#[test]
fn higher_ranked_parameter_attributes_discover_their_blocks() {
    let (found, _) = sites("fn first(value: for<#[inspect = { 1 + 1 }] 'a> fn(&'a u8)) {}");
    assert_eq!(found.len(), 2);
    assert_eq!(forms_of(&found, ContextKind::Block).len(), 1);
    assert!(found.iter().all(|site| site.item == "first"));
    let (renamed, _) = sites("fn second(other: for<#[inspect = { 1 + 1 }] 'b> fn(&'b u8)) {}");
    assert_eq!(
        forms_of(&found, ContextKind::Block),
        forms_of(&renamed, ContextKind::Block)
    );
}

#[test]
fn guarded_pattern_conditions_discover_their_nested_blocks() {
    let (a, _) = sites(
        "fn first(value: Option<u32>) -> u32 {
             match value {
                 _ if let Some(x) = value && accept({ x + x }) => 0,
                 _ => 1,
             }
         }",
    );
    let (b, _) = sites(
        "fn second(other: Option<u32>) -> u32 {
             match other {
                 _ if let Some(y) = other && accept({ y + y }) => 0,
                 _ => 1,
             }
         }",
    );
    assert_eq!(forms_of(&a, ContextKind::Block).len(), 1);
    assert_eq!(
        forms_of(&a, ContextKind::Block),
        forms_of(&b, ContextKind::Block)
    );
    let (different, _) = sites(
        "fn third(value: Option<u32>) -> u32 {
             match value {
                 _ if let Some(x) = value && accept({ x + value.unwrap() }) => 0,
                 _ => 1,
             }
         }",
    );
    assert_ne!(
        forms_of(&a, ContextKind::Block),
        forms_of(&different, ContextKind::Block)
    );
}
