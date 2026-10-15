//! Alpha-rename analysis pass fixtures, one per missing coverage line group.

/// The canonical view of one indexed declaration, by its written name.
fn view_of(file: &syn::File, name: &str) -> syn_canon::CanonicalForm {
    let context = syn_canon::SourceContext::new(core::iter::once((&[][..], file)));
    let target = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(f) if f.sig.ident == name => Some((&f.sig, f.block.as_ref())),
            syn::Item::Impl(imp) => imp.items.iter().find_map(|member| match member {
                syn::ImplItem::Fn(f) if f.sig.ident == name => Some((&f.sig, &f.block)),
                _ => None,
            }),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{name} fixture"));
    context.function(target.0, target.1).unwrap().canonicalize()
}

#[test]
fn seeded_helper_views_keep_module_routes_apart() {
    let first: syn::File = syn::parse_str(
        "fn helper() -> i32 { 3 }
         fn target() -> i32 { helper() }",
    )
    .unwrap();
    let twin: syn::File = syn::parse_str(
        "fn helper_2() -> i32 { 3 }
         fn target_2() -> i32 { helper_2() }",
    )
    .unwrap();
    let ghost: syn::File = syn::parse_str(
        "fn ghost() -> i32 { 3 }
         fn target_3() -> i32 { missing() }",
    )
    .unwrap();
    let a = view_of(&first, "target");
    let b = view_of(&twin, "target_2");
    let c = view_of(&ghost, "target_3");
    assert_ne!(a, b);
    assert_ne!(a, c);
}

#[test]
fn self_references_keep_the_owner_route_across_spelling_drift() {
    let first: syn::File = syn::parse_str(
        "struct S;
         impl S { fn make() -> Self { Self } }",
    )
    .unwrap();
    let twin: syn::File = syn::parse_str(
        "struct T;
         impl T { fn make() -> Self { Self } }",
    )
    .unwrap();
    assert_eq!(
        syn_canon::canonicalize(first),
        syn_canon::canonicalize(twin)
    );
}

#[test]
fn seeded_views_prove_an_inherited_owner_route_for_a_nested_local_impl() {
    let first: syn::File = syn::parse_str(
        "struct S;
         fn holder() {
             impl S { fn make() -> Self { Self } }
         }",
    )
    .unwrap();
    let renamed_holder: syn::File = syn::parse_str(
        "struct S;
         fn holder() {
             impl S { fn make() -> Self { Self } }
         }",
    )
    .unwrap();
    let twin: syn::File = syn::parse_str(
        "struct T;
         fn holder() {
             impl T { fn make() -> Self { Self } }
         }",
    )
    .unwrap();
    assert_eq!(
        view_of(&first, "holder"),
        view_of(&renamed_holder, "holder")
    );
    assert_ne!(view_of(&first, "holder"), view_of(&twin, "holder"));
}

#[test]
fn use_alias_chains_keep_the_module_step_routes_across_spelling_drift() {
    let first: syn::File = syn::parse_str(
        "mod m {
            pub mod n {
                pub struct Item;
            }
            pub struct Point;
        }
        use m::n as pa;
        use pa::gone as xa;
        use m::n as xb;
        use m::Point as xc;
        use m as xd;
        fn make_a() { let _ = xa::y; }
        fn make_b() -> xb::Item { loop {} }
        fn make_c() { let _ = xc::origin; }
        fn make_d() -> xd::Point { loop {} }",
    )
    .unwrap();
    let twin: syn::File = syn::parse_str(
        "mod m {
            pub mod n {
                pub struct Item;
            }
            pub struct Point;
        }
        use m::n as pa;
        use pa::gone as za;
        use m::n as zb;
        use m::Point as zc;
        use m as zd;
        fn take_a() { let _ = za::y; }
        fn take_b() -> zb::Item { loop {} }
        fn take_c() { let _ = zc::origin; }
        fn take_d() -> zd::Point { loop {} }",
    )
    .unwrap();
    assert_eq!(
        syn_canon::canonicalize(first),
        syn_canon::canonicalize(twin)
    );
}

#[test]
fn leading_colon_self_types_stay_unproven_across_spelling_drift() {
    let unproven: syn::File = syn::parse_str(
        r#"struct S;
        impl fmt::Debug for ::S {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("s")
            }
        }"#,
    )
    .unwrap();
    let proven: syn::File = syn::parse_str(
        r#"struct S;
        impl fmt::Debug for S {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("s")
            }
        }"#,
    )
    .unwrap();
    assert_ne!(
        syn_canon::canonicalize(unproven),
        syn_canon::canonicalize(proven)
    );
}

#[test]
fn self_prefixed_self_types_stay_unproven_across_spelling_drift() {
    let unproven: syn::File = syn::parse_str(
        r#"struct S;
        impl fmt::Debug for self::S {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("s")
            }
        }"#,
    )
    .unwrap();
    let proven: syn::File = syn::parse_str(
        r#"struct S;
        impl fmt::Debug for S {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("s")
            }
        }"#,
    )
    .unwrap();
    assert_ne!(
        syn_canon::canonicalize(unproven),
        syn_canon::canonicalize(proven)
    );
}

#[test]
fn rooted_import_targets_keep_the_owner_route_unproven() {
    let rooted: syn::File = syn::parse_str(
        r#"struct S;
        use ::S as T;
        impl fmt::Debug for T {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("s")
            }
        }"#,
    )
    .unwrap();
    let bare: syn::File = syn::parse_str(
        r#"struct S;
        use S as T;
        impl fmt::Debug for T {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("s")
            }
        }"#,
    )
    .unwrap();
    assert_ne!(
        syn_canon::canonicalize(rooted),
        syn_canon::canonicalize(bare)
    );
}

#[test]
fn module_terminals_keep_the_owner_route_unproven() {
    let first: syn::File = syn::parse_str(
        r#"mod m {
            pub mod sub {}
        }
        impl fmt::Debug for m::sub {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("s")
            }
        }"#,
    )
    .unwrap();
    let twin: syn::File = syn::parse_str(
        r#"mod mm {
            pub mod inner {}
        }
        impl fmt::Debug for mm::inner {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("s")
            }
        }"#,
    )
    .unwrap();
    assert_eq!(
        syn_canon::canonicalize(first),
        syn_canon::canonicalize(twin)
    );
}

#[test]
fn module_named_self_types_keep_the_owner_route_unproven() {
    let first: syn::File = syn::parse_str(
        r#"mod m {}
        impl fmt::Debug for m {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("s")
            }
        }"#,
    )
    .unwrap();
    let twin: syn::File = syn::parse_str(
        r#"mod n {}
        impl fmt::Debug for n {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("s")
            }
        }"#,
    )
    .unwrap();
    assert_eq!(
        syn_canon::canonicalize(first),
        syn_canon::canonicalize(twin)
    );
}

#[test]
fn unmatched_crate_root_macro_paths_stay_opaque_across_spelling_drift() {
    let first: syn::File = syn::parse_str(
        r#"use std as s;
        fn first() {
            let _ = alloc::format!("{}", 1);
        }
        fn renamed() {
            let _ = s::format!("{}", 1);
        }"#,
    )
    .unwrap();
    let twin: syn::File = syn::parse_str(
        r#"use std as ss;
        fn second() {
            let _ = alloc::format!("{}", 1);
        }
        fn renamed_2() {
            let _ = ss::format!("{}", 1);
        }"#,
    )
    .unwrap();
    assert_eq!(
        syn_canon::canonicalize(first),
        syn_canon::canonicalize(twin)
    );
}
