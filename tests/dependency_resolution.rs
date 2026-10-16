//! Original declaration and inherited reference identities.

use syn_canon::SourceContext;

fn function<'a>(file: &'a syn::File, name: &str) -> &'a syn::ItemFn {
    file.items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == name => Some(function),
            _ => None,
        })
        .unwrap()
}

fn function_form(context: &SourceContext<'_>, function: &syn::ItemFn) -> syn_canon::CanonicalForm {
    context
        .function(&function.sig, &function.block)
        .unwrap()
        .canonicalize()
}

#[test]
fn function_views_require_the_exact_original_declaration() {
    let file: syn::File =
        syn::parse_str("fn a(input: u32) -> u32 { input } fn b(value: u32) -> u32 { value }")
            .unwrap();
    let context = SourceContext::new(core::iter::once((&[][..], &file)));
    let a = function(&file, "a");
    let b = function(&file, "b");
    let clone = a.clone();
    let original = context.function(&a.sig, &a.block).unwrap().canonicalize();
    assert_eq!(
        original,
        context.function(&b.sig, &b.block).unwrap().canonicalize()
    );
    assert!(context.function(&clone.sig, &clone.block).is_none());
    assert!(context.function(&a.sig, &b.block).is_none());
}

#[test]
fn contextual_method_calls_retain_inherited_helper_identity() {
    let source: syn::File = syn::parse_str(
        "
        struct First;
        struct Second;
        impl First {
            fn helper() -> u32 { 1u32 }
            fn caller() -> u32 { Self::helper() }
        }
        impl Second {
            fn helper() -> u32 { 2u32 }
            fn caller() -> u32 { Self::helper() }
        }
    ",
    )
    .unwrap();
    let context = SourceContext::new(core::iter::once((&[][..], &source)));
    let callers: Vec<_> = source
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Impl(item) => item.items.iter().find_map(|member| match member {
                syn::ImplItem::Fn(function) if function.sig.ident == "caller" => Some(
                    context
                        .function(&function.sig, &function.block)
                        .unwrap()
                        .canonicalize(),
                ),
                _ => None,
            }),
            _ => None,
        })
        .collect();
    assert_ne!(callers[0], callers[1]);
}

#[test]
fn imported_helpers_retain_their_qualified_definition_identity() {
    let source: syn::File = syn::parse_str(
        "
        mod a { pub fn helper() -> u32 { 1u32 } }
        mod b { pub fn helper() -> u32 { 2u32 } }
        use crate::a::helper as first;
        use crate::b::helper as second;
        fn left() -> u32 { first() }
        fn right() -> u32 { second() }
        fn same() -> u32 { first() }
        ",
    )
    .unwrap();
    let context = SourceContext::new(core::iter::once((&[][..], &source)));
    let canonical = |name| {
        let function = function(&source, name);
        context
            .function(&function.sig, &function.block)
            .unwrap()
            .canonicalize()
    };
    assert_ne!(canonical("left"), canonical("right"));
    assert_eq!(canonical("left"), canonical("same"));
}

#[test]
fn inherited_generic_dispatch_retains_its_enclosing_definition() {
    let source: syn::File = syn::parse_str(
        "
        trait LeftFactory { fn value() -> u32; }
        trait RightFactory { fn value() -> u32; }
        struct Left<T>(T);
        struct Right<T>(T);
        impl<T: LeftFactory> Left<T> {
            fn first() -> u32 { T::value() }
            fn same() -> u32 { T::value() }
        }
        impl<T: RightFactory> Right<T> {
            fn second() -> u32 { T::value() }
        }
        ",
    )
    .unwrap();
    let context = SourceContext::new(core::iter::once((&[][..], &source)));
    let forms: Vec<_> = source
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Impl(owner) => Some(&owner.items),
            _ => None,
        })
        .flatten()
        .filter_map(|item| match item {
            syn::ImplItem::Fn(function) => Some(
                context
                    .function(&function.sig, &function.block)
                    .unwrap()
                    .canonicalize(),
            ),
            _ => None,
        })
        .collect();
    assert_eq!(forms[0], forms[1]);
    assert_ne!(forms[0], forms[2]);
}

#[test]
fn local_binding_names_do_not_inherit_shadowed_module_origins() {
    let source: syn::File = syn::parse_str(
        "fn x()->u32 { 9u32 }
        fn original()->u32 { let x=1u32; x }
        fn renamed()->u32 { let y=1u32; y }",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let original = function(&source, "original");
    let renamed = function(&source, "renamed");
    assert_eq!(
        context
            .function(&original.sig, &original.block)
            .unwrap()
            .canonicalize(),
        context
            .function(&renamed.sig, &renamed.block)
            .unwrap()
            .canonicalize()
    );
}

#[test]
fn generated_names_cannot_capture_resolved_inherited_helpers() {
    let source: syn::File = syn::parse_str(
        "macro_rules! define {
            () => { fn __dejadoc_inherited_6669727374()->u32 { 2u32 } };
        }
        define!();
        fn first()->u32 { 1u32 }
        fn resolved()->u32 { first() }
        fn generated()->u32 { __dejadoc_inherited_6669727374() }",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let resolved = function(&source, "resolved");
    let generated = function(&source, "generated");
    assert_ne!(
        context
            .function(&resolved.sig, &resolved.block)
            .unwrap()
            .canonicalize(),
        context
            .function(&generated.sig, &generated.block)
            .unwrap()
            .canonicalize()
    );
}

fn impl_method<'a>(
    context: &'a SourceContext<'a>,
    source: &'a syn::File,
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

fn module_function<'a>(
    context: &'a SourceContext<'a>,
    source: &'a syn::File,
    name: &str,
) -> syn_canon::CanonicalForm {
    source
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Mod(mod_item) => mod_item.content.as_ref().and_then(|(_, items)| {
                items.iter().find_map(|inner| match inner {
                    syn::Item::Fn(function) if function.sig.ident == name => Some(
                        context
                            .function(&function.sig, &function.block)
                            .unwrap()
                            .canonicalize(),
                    ),
                    _ => None,
                })
            }),
            _ => None,
        })
        .unwrap()
}

#[test]
fn owner_self_names_resolve_through_the_module_environment() {
    let source: syn::File = syn::parse_str(
        "
        struct First;
        use First as Here;
        struct With_Under;
        use With_Under as Alias;
        impl First { fn left() -> u32 { Self::make() } fn make() -> u32 { 1u32 } }
        impl Here { fn right() -> u32 { Self::make() } fn make() -> u32 { 1u32 } }
        impl With_Under { fn low() -> u32 { Self::mark() } fn mark() -> u32 { 1u32 } }
        impl Alias { fn high() -> u32 { Self::mark() } fn mark() -> u32 { 1u32 } }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    assert_eq!(
        impl_method(&context, &source, "left"),
        impl_method(&context, &source, "right")
    );
    assert_eq!(
        impl_method(&context, &source, "low"),
        impl_method(&context, &source, "high")
    );
}

#[test]
fn owner_self_names_mangle_compound_owner_types() {
    let source: syn::File = syn::parse_str(
        "
        struct Pair<U>(U);
        impl Pair<u8> { fn left() -> u32 { Self::make() } fn make() -> u32 { 1u32 } }
        impl Pair<u16> { fn right() -> u32 { Self::make() } fn make() -> u32 { 1u32 } }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    assert_ne!(
        impl_method(&context, &source, "left"),
        impl_method(&context, &source, "right")
    );
}

#[test]
fn external_primitive_crate_imports_share_one_spelling() {
    let source: syn::File = syn::parse_str(
        "
        struct Marker;
        use std::convert::identity as first;
        use std::convert::identity as second;
        fn left() -> u32 { first(7u32) }
        fn right() -> u32 { second(7u32) }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let canonical = |name| {
        let function = function(&source, name);
        context
            .function(&function.sig, &function.block)
            .unwrap()
            .canonicalize()
    };
    assert_eq!(canonical("left"), canonical("right"));
}

#[test]
fn renamed_primitive_crate_name_shadows_the_external_spelling() {
    let source: syn::File = syn::parse_str(
        "
        use core as std;
        use std::convert::identity as first;
        use std::convert::identity as second;
        fn left() -> u32 { first(7u32) }
        fn right() -> u32 { second(7u32) }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let canonical = |name| {
        let function = function(&source, name);
        context
            .function(&function.sig, &function.block)
            .unwrap()
            .canonicalize()
    };
    assert_ne!(canonical("left"), canonical("right"));
}

#[test]
fn raw_spelled_struct_shadows_the_primitive_crates() {
    let source: syn::File = syn::parse_str(
        "
        struct r#std;
        use std::helper as first;
        use std::helper as second;
        fn left() -> u32 { first(7u32) }
        fn right() -> u32 { second(7u32) }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let canonical = |name| {
        let function = function(&source, name);
        context
            .function(&function.sig, &function.block)
            .unwrap()
            .canonicalize()
    };
    assert_ne!(canonical("left"), canonical("right"));
}

#[test]
fn leading_colon_imports_keep_the_external_spelling() {
    let source: syn::File = syn::parse_str(
        "
        fn value() -> u32 { 1u32 }
        use ::value as alias;
        fn left() -> u32 { alias(7u32) }
        fn right() -> u32 { value(7u32) }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let canonical = |name| {
        let function = function(&source, name);
        context
            .function(&function.sig, &function.block)
            .unwrap()
            .canonicalize()
    };
    assert_eq!(canonical("left"), canonical("right"));
}

#[test]
fn crate_qualified_imports_resolve_through_the_module_map() {
    let source: syn::File = syn::parse_str(
        "
        mod a { pub fn helper() -> u32 { 1u32 } }
        use crate::a::helper as first;
        use a::helper as second;
        fn left() -> u32 { first() }
        fn right() -> u32 { second() }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let canonical = |name| {
        let function = function(&source, name);
        context
            .function(&function.sig, &function.block)
            .unwrap()
            .canonicalize()
    };
    assert_eq!(canonical("left"), canonical("right"));
}

#[test]
fn self_qualified_imports_resolve_through_the_module_map() {
    let source: syn::File = syn::parse_str(
        "
        mod a { pub fn helper() -> u32 { 1u32 } }
        use self::a::helper as first;
        use a::helper as second;
        fn left() -> u32 { first() }
        fn right() -> u32 { second() }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let canonical = |name| {
        let function = function(&source, name);
        context
            .function(&function.sig, &function.block)
            .unwrap()
            .canonicalize()
    };
    assert_eq!(canonical("left"), canonical("right"));
}

#[test]
fn super_qualified_imports_resolve_from_the_importing_module() {
    let source: syn::File = syn::parse_str(
        "
        mod a { pub fn helper() -> u32 { 1u32 } }
        mod left_module { use super::a::helper as first; fn left() -> u32 { first() } }
        mod right_module { use super::a::helper as second; fn right() -> u32 { second() } }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    assert_eq!(
        module_function(&context, &source, "left"),
        module_function(&context, &source, "right")
    );
}

#[test]
fn same_module_uses_resolve_to_their_local_values() {
    let pair = |source: &syn::File| {
        let context = SourceContext::new([(&[][..], source)]);
        let canonical = |name| {
            let function = function(source, name);
            context
                .function(&function.sig, &function.block)
                .unwrap()
                .canonicalize()
        };
        (canonical("left"), canonical("right"))
    };
    let (left, right) = pair(
        &syn::parse_str(
            "
            fn helper() -> u32 { 1u32 }
            use helper as alias;
            fn left() -> u32 { alias() }
            fn right() -> u32 { helper() }
        ",
        )
        .unwrap(),
    );
    assert_eq!(left, right);
    let (left, right) = pair(
        &syn::parse_str(
            "
            const helper: u32 = 1u32;
            use helper as alias;
            fn left() -> u32 { alias }
            fn right() -> u32 { helper }
        ",
        )
        .unwrap(),
    );
    assert_eq!(left, right);
    let (left, right) = pair(
        &syn::parse_str(
            "
            static helper: u32 = 1u32;
            use helper as alias;
            fn left() -> u32 { alias }
            fn right() -> u32 { helper }
        ",
        )
        .unwrap(),
    );
    assert_eq!(left, right);
}

#[test]
fn aliased_uses_resolve_to_their_single_module_value() {
    let pair = |source: &syn::File| {
        let context = SourceContext::new([(&[][..], source)]);
        let canonical = |name| {
            let function = function(source, name);
            context
                .function(&function.sig, &function.block)
                .unwrap()
                .canonicalize()
        };
        (canonical("left"), canonical("right"))
    };
    let (left, right) = pair(
        &syn::parse_str(
            "
            mod a { pub fn helper() -> u32 { 1u32 } }
            use a::helper as first;
            use a::helper as second;
            fn left() -> u32 { first() }
            fn right() -> u32 { second() }
        ",
        )
        .unwrap(),
    );
    assert_eq!(left, right);
    let (left, right) = pair(
        &syn::parse_str(
            "
            mod a { pub const helper: u32 = 1u32; }
            use a::helper as first;
            use a::helper as second;
            fn left() -> u32 { first }
            fn right() -> u32 { second }
        ",
        )
        .unwrap(),
    );
    assert_eq!(left, right);
    let (left, right) = pair(
        &syn::parse_str(
            "
            mod a { pub static helper: u32 = 1u32; }
            use a::helper as first;
            use a::helper as second;
            fn left() -> u32 { first }
            fn right() -> u32 { second }
        ",
        )
        .unwrap(),
    );
    assert_eq!(left, right);
}

#[test]
fn imported_type_items_resolve_to_their_local_definitions() {
    let pair = |source: &syn::File| {
        let context = SourceContext::new([(&[][..], source)]);
        let canonical = |name| {
            let function = function(source, name);
            context
                .function(&function.sig, &function.block)
                .unwrap()
                .canonicalize()
        };
        (canonical("left"), canonical("right"))
    };
    for declaration in [
        "struct helper;",
        "enum helper {}",
        "union helper { field: u32 }",
        "type helper = u32;",
        "trait helper {}",
        "trait helper = Copy;",
        "mod helper {}",
    ] {
        let source: syn::File = syn::parse_str(
            format!(
                "
                {declaration}
                use helper as first;
                use helper as second;
                fn left() -> first {{ unimplemented!() }}
                fn right() -> helper {{ unimplemented!() }}
            "
            )
            .as_str(),
        )
        .unwrap();
        let (left, right) = pair(&source);
        assert_eq!(left, right);
    }
}

#[test]
fn macro_calls_resolve_to_their_defining_module() {
    let source: syn::File = syn::parse_str(
        "
        mod left_module { macro_rules! helper { () => { 1u32 } } fn left() -> u32 { helper!() } }
        mod right_module { macro_rules! helper { () => { 1u32 } } fn right() -> u32 { helper!() } }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    assert_ne!(
        module_function(&context, &source, "left"),
        module_function(&context, &source, "right")
    );
}

#[test]
fn external_item_named_like_a_primitive_crate_shadows_it() {
    let source: syn::File = syn::parse_str(
        "
        use some_crate::std;
        use std::convert::identity as first;
        use std::convert::identity as second;
        fn left() -> u32 { first(7u32) }
        fn right() -> u32 { second(7u32) }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let canonical = |name| {
        let function = function(&source, name);
        context
            .function(&function.sig, &function.block)
            .unwrap()
            .canonicalize()
    };
    assert_ne!(canonical("left"), canonical("right"));
}

#[test]
fn self_named_imports_shadow_the_primitive_crate() {
    let source: syn::File = syn::parse_str(
        "
        use std::self;
        use std::convert::identity as first;
        use std::convert::identity as second;
        fn left() -> u32 { first(7u32) }
        fn right() -> u32 { second(7u32) }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let canonical = |name| {
        let function = function(&source, name);
        context
            .function(&function.sig, &function.block)
            .unwrap()
            .canonicalize()
    };
    assert_ne!(canonical("left"), canonical("right"));
}

#[test]
fn seeded_item_ids_keep_the_helpers_distinct() {
    let source: syn::File = syn::parse_str(
        "
        fn first() -> u32 { 1u32 }
        fn second() -> u32 { 2u32 }
        fn left() -> u32 { first() }
        fn right() -> u32 { second() }
        fn same() -> u32 { first() }
        fn renamed() -> u32 { second() }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let canonical = |name| {
        let function = function(&source, name);
        context
            .function(&function.sig, &function.block)
            .unwrap()
            .canonicalize()
    };
    assert_eq!(canonical("left"), canonical("same"));
    assert_ne!(canonical("left"), canonical("right"));
    assert_ne!(canonical("left"), canonical("renamed"));
}

#[test]
fn generic_type_ids_keep_the_parameters_distinct() {
    let source: syn::File = syn::parse_str(
        "
        struct Owner<T, U>(T, U);
        impl<T, U> Owner<T, U> {
            fn left() -> T { unimplemented!() }
            fn right() -> U { unimplemented!() }
            fn same() -> T { unimplemented!() }
        }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    assert_eq!(
        impl_method(&context, &source, "left"),
        impl_method(&context, &source, "same")
    );
    assert_ne!(
        impl_method(&context, &source, "left"),
        impl_method(&context, &source, "right")
    );
}

#[test]
fn generic_const_ids_keep_the_parameters_distinct() {
    let source: syn::File = syn::parse_str(
        "
        struct Holder<const A: usize, const B: usize>;
        impl<const A: usize, const B: usize> Holder<A, B> {
            fn left() -> A { unimplemented!() }
            fn right() -> B { unimplemented!() }
            fn same() -> A { unimplemented!() }
        }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    assert_eq!(
        impl_method(&context, &source, "left"),
        impl_method(&context, &source, "same")
    );
    assert_ne!(
        impl_method(&context, &source, "left"),
        impl_method(&context, &source, "right")
    );
}

#[test]
fn descending_generic_type_ids_do_not_capture_seed_items() {
    let source: syn::File = syn::parse_str(
        "
        struct Owner<T, U>(T, U);
        impl<T, U> Owner<T, U> {
            fn left() -> Owner { unimplemented!() }
            fn right() -> U { unimplemented!() }
        }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    assert_ne!(
        impl_method(&context, &source, "left"),
        impl_method(&context, &source, "right")
    );
}

#[test]
fn descending_generic_const_ids_do_not_capture_seed_items() {
    let source: syn::File = syn::parse_str(
        "
        struct Holder<const A: usize, const B: usize>(A, B);
        impl<const A: usize, const B: usize> Holder<A, B> {
            fn left() -> Holder { unimplemented!() }
            fn right() -> B { unimplemented!() }
        }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    assert_ne!(
        impl_method(&context, &source, "left"),
        impl_method(&context, &source, "right")
    );
}

#[test]
fn macro_literal_owner_spaces_keep_the_owners_distinct() {
    let source: syn::File = syn::parse_str(
        r#"
        macro_rules! selected {
            ("one space") => { First };
            ("one  space") => { Second };
        }
        impl selected!("one space") { fn left() -> u32 { Self::value() } fn value() -> u32 { 1u32 } }
        impl selected!("one  space") { fn right() -> u32 { Self::value() } fn value() -> u32 { 1u32 } }
        impl selected!("one space") { fn renamed() -> u32 { Self::value() } fn value() -> u32 { 1u32 } }
    "#,
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    assert_eq!(
        impl_method(&context, &source, "left"),
        impl_method(&context, &source, "renamed")
    );
    assert_ne!(
        impl_method(&context, &source, "left"),
        impl_method(&context, &source, "right")
    );
}

#[test]
fn shared_alias_imports_bind_each_namespace_to_its_own_target() {
    let source: syn::File = syn::parse_str(
        "
        mod local {
            pub struct Thing { pub value: u32 }
            pub fn one() -> u32 { 1u32 }
            pub fn two() -> u32 { 2u32 }
        }
        mod first {
            use crate::local::{Thing as Shared, one as Shared};
            fn left() -> u32 { Shared() }
            fn renamed() -> u32 { Shared() }
        }
        mod second {
            use crate::local::{Thing as Shared, two as Shared};
            fn right() -> u32 { Shared() }
        }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    assert_ne!(
        module_function(&context, &source, "left"),
        module_function(&context, &source, "right")
    );
    assert_eq!(
        module_function(&context, &source, "left"),
        module_function(&context, &source, "renamed")
    );
}

#[test]
fn root_super_imports_resolve_to_their_own_aliases() {
    let source: syn::File = syn::parse_str(
        "
        use super::value as first;
        use super::value as second;
        fn left() -> u32 { first(7u32) }
        fn right() -> u32 { second(7u32) }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    assert_ne!(
        function_form(&context, function(&source, "left")),
        function_form(&context, function(&source, "right"))
    );
}

#[test]
fn glob_imports_do_not_shadow_the_primitive_crates() {
    let source: syn::File = syn::parse_str(
        "
        use some_crate::*;
        use core::convert::identity as first;
        use core::convert::identity as second;
        fn left() -> u32 { first(7u32) }
        fn right() -> u32 { second(7u32) }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    assert_eq!(
        function_form(&context, function(&source, "left")),
        function_form(&context, function(&source, "right"))
    );
}

#[test]
fn non_function_impl_members_leave_the_method_forms_alone() {
    let source: syn::File = syn::parse_str(
        "
        struct First;
        impl First {
            const MARK: u32 = 1u32;
            fn left() -> u32 { Self::MARK + 1u32 }
            fn same() -> u32 { Self::MARK + 1u32 }
        }
        struct Second;
        impl Second {
            const MARK: u32 = 1u32;
            fn right() -> u32 { Self::MARK + 1u32 }
        }
    ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    assert_eq!(
        impl_method(&context, &source, "left"),
        impl_method(&context, &source, "same")
    );
    assert_ne!(
        impl_method(&context, &source, "left"),
        impl_method(&context, &source, "right")
    );
}

#[test]
fn super_from_an_unindexed_parent_module_stays_unresolved() {
    let source: syn::File = syn::parse_str(
        "
        use super::value as first;
        use super::other as second;
        fn left() -> u32 { first(7u32) }
        fn right() -> u32 { second(7u32) }
    ",
    )
    .unwrap();
    let path = ["a".to_string()];
    let context = SourceContext::new([(&path[..], &source)]);
    assert_ne!(
        function_form(&context, function(&source, "left")),
        function_form(&context, function(&source, "right"))
    );
}

#[test]
fn shared_alias_type_references_ignore_the_value_import() {
    let source: syn::File = syn::parse_str(
        "
        mod local {
            pub struct Thing { pub value: u32 }
            pub struct Other { pub value: u32 }
            pub fn one() -> u32 { 1 }
            pub fn two() -> u32 { 2 }
        }
        mod first {
            use crate::local::{one as Shared, Thing as Shared};
            fn left() -> Shared { Shared { value: 7 } }
        }
        mod second {
            use crate::local::two as Renamed;
            use crate::local::Thing as Renamed;
            fn same() -> Renamed { Renamed { value: 7 } }
        }
        mod third {
            use crate::local::{one as Shared, Other as Shared};
            fn right() -> Shared { Shared { value: 7 } }
        }
        ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let left = module_function(&context, &source, "left");
    assert_eq!(left, module_function(&context, &source, "same"));
    assert_ne!(left, module_function(&context, &source, "right"));
}

#[test]
fn imported_unit_constructor_patterns_retain_their_reference_role() {
    let source: syn::File = syn::parse_str(
        "
        mod local { pub struct Marker; }
        mod callers {
            use crate::local::{Marker as First, Marker as Second};
            fn left(value: First) -> bool { matches!(value, First) }
            fn same(renamed: Second) -> bool { matches!(renamed, Second) }
            fn binding(value: First) -> bool { matches!(value, variable) }
        }
        ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let left = module_function(&context, &source, "left");
    assert_eq!(left, module_function(&context, &source, "same"));
    assert_ne!(left, module_function(&context, &source, "binding"));
}

#[test]
fn an_import_can_supply_type_value_and_macro_namespaces() {
    let source: syn::File = syn::parse_str(
        "
        pub struct Shared { pub value: u32 }
        pub fn Shared() -> u32 { 7 }
        #[macro_export]
        macro_rules! Shared { () => { 9u32 }; }
        mod callers {
            use crate::{Shared as First, Shared as Second};
            fn type_left() -> First { First { value: 7 } }
            fn type_same() -> Second { Second { value: 7 } }
            fn value_left() -> u32 { First() }
            fn value_same() -> u32 { Second() }
            fn macro_left() -> u32 { First!() }
            fn macro_same() -> u32 { Second!() }
        }
        ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    for (left, same) in [
        ("type_left", "type_same"),
        ("value_left", "value_same"),
        ("macro_left", "macro_same"),
    ] {
        assert_eq!(
            module_function(&context, &source, left),
            module_function(&context, &source, same)
        );
    }
}

#[test]
fn unknown_imports_leave_local_value_declarations_intact() {
    let source: syn::File = syn::parse_str(
        "
        mod first {
            pub fn Shared() -> u32 { 1 }
            use ::dependency::Thing as Shared;
            fn left() -> u32 { Shared() }
            fn same() -> u32 { Shared() }
        }
        mod second {
            use ::dependency::Thing as Shared;
            pub fn Shared() -> u32 { 2 }
            fn right() -> u32 { Shared() }
        }
        ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let left = module_function(&context, &source, "left");
    assert_eq!(left, module_function(&context, &source, "same"));
    assert_ne!(left, module_function(&context, &source, "right"));
}

#[test]
fn ambiguous_unknown_imports_keep_the_alias_scoped_and_opaque() {
    let source: syn::File = syn::parse_str(
        "
        mod first {
            use ::dependency::{Thing as Shared, one as Shared};
            fn left() -> u32 { Shared() }
            fn same() -> u32 { Shared() }
        }
        mod second {
            use ::dependency::Thing as Shared;
            use ::dependency::two as Shared;
            fn right() -> u32 { Shared() }
        }
        ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let left = module_function(&context, &source, "left");
    assert_eq!(left, module_function(&context, &source, "same"));
    assert_ne!(left, module_function(&context, &source, "right"));
}

#[test]
fn opaque_macro_arguments_retain_all_inherited_namespace_targets() {
    let source: syn::File = syn::parse_str(
        "
        pub struct Thing { pub value: u32 }
        pub struct Other { pub value: u32 }
        impl Thing { pub fn value() -> u32 { 1 } }
        impl Other { pub fn value() -> u32 { 2 } }
        pub fn helper() -> u32 { 0 }
        #[macro_export]
        macro_rules! choose { ($ty:ty) => { <$ty>::value() }; }
        mod first {
            use crate::{Thing as Shared, helper as Shared};
            fn left() -> u32 { crate::choose!(Shared) }
            fn same() -> u32 { crate::choose!(Shared) }
        }
        mod second {
            use crate::{Other as Shared, helper as Shared};
            fn right() -> u32 { crate::choose!(Shared) }
        }
        ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let left = module_function(&context, &source, "left");
    assert_eq!(left, module_function(&context, &source, "same"));
    assert_ne!(left, module_function(&context, &source, "right"));
}

#[test]
fn struct_patterns_and_associated_paths_read_the_type_namespace() {
    let source: syn::File = syn::parse_str(
        "
        mod local {
            pub struct Thing { pub value: u32 }
            impl Thing { pub fn read() -> u32 { 7 } }
            pub fn one() -> u32 { 1 }
            pub fn two() -> u32 { 2 }
        }
        mod first {
            use crate::local::{Thing as Shared, one as Shared};
            fn pat_left(input: Shared) -> u32 { let Shared { value } = input; value }
            fn path_left() -> u32 { Shared::read() }
        }
        mod second {
            use crate::local::{Thing as Renamed, two as Renamed};
            fn pat_same(renamed: Renamed) -> u32 { let Renamed { value } = renamed; value }
            fn path_same() -> u32 { Renamed::read() }
        }
        ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    for (left, same) in [("pat_left", "pat_same"), ("path_left", "path_same")] {
        assert_eq!(
            module_function(&context, &source, left),
            module_function(&context, &source, same)
        );
    }
}

#[test]
fn qualified_macro_paths_resolve_the_imported_module_namespace() {
    let source: syn::File = syn::parse_str(
        "
        #[macro_export]
        macro_rules! one { () => { 1u32 }; }
        #[macro_export]
        macro_rules! two { () => { 2u32 }; }
        mod first_macros { pub use crate::one as helper; }
        mod second_macros { pub use crate::two as helper; }
        mod callers {
            use crate::{first_macros as First, first_macros as Same, second_macros as Second};
            fn left() -> u32 { First::helper!() }
            fn same() -> u32 { Same::helper!() }
            fn right() -> u32 { Second::helper!() }
        }
        ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let left = module_function(&context, &source, "left");
    assert_eq!(left, module_function(&context, &source, "same"));
    assert_ne!(left, module_function(&context, &source, "right"));
}

#[test]
fn opaque_raw_imports_retain_their_qualified_targets_and_spelling() {
    let source: syn::File = syn::parse_str(
        "
        pub fn one() -> u32 { 1 }
        pub fn two() -> u32 { 2 }
        #[macro_export]
        macro_rules! call { ($value:ident) => { $value() }; }
        mod first {
            use crate::one as r#Shared;
            fn left() -> u32 { crate::call!(Shared) }
            fn same() -> u32 { crate::call!(Shared) }
            fn raw() -> u32 { crate::call!(r#Shared) }
        }
        mod second {
            use crate::two as r#Shared;
            fn right() -> u32 { crate::call!(Shared) }
        }
        ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let left = module_function(&context, &source, "left");
    assert_eq!(left, module_function(&context, &source, "same"));
    assert_ne!(left, module_function(&context, &source, "right"));
    assert_ne!(left, module_function(&context, &source, "raw"));
}

#[test]
fn opaque_single_namespace_imports_preserve_raw_binding_and_reference_spelling() {
    let source: syn::File = syn::parse_str(
        "
        pub fn helper() -> u32 { 1 }
        #[macro_export]
        macro_rules! call { ($value:ident) => { $value() }; }
        mod plain {
            use crate::helper as one_alias;
            use crate::helper as two_alias;
            fn left() -> u32 { crate::call!(one_alias) }
            fn same() -> u32 { crate::call!(two_alias) }
        }
        mod raw_binding {
            use crate::helper as r#Shared;
            fn declared_raw() -> u32 { crate::call!(Shared) }
        }
        mod raw_reference {
            use crate::helper as Shared;
            fn written_raw() -> u32 { crate::call!(r#Shared) }
        }
        ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let left = module_function(&context, &source, "left");
    assert_eq!(left, module_function(&context, &source, "same"));
    assert_ne!(left, module_function(&context, &source, "declared_raw"));
    assert_ne!(left, module_function(&context, &source, "written_raw"));
}

#[test]
fn opaque_single_targets_share_their_qualified_identity_across_namespace_views() {
    let type_only: syn::File = syn::parse_str(
        "
        mod m { pub type Shared = u32; }
        use m::Shared;
        macro_rules! opaque { ($name:ident) => { 1u32 }; }
        fn f() -> u32 { opaque!(Shared) }
        ",
    )
    .unwrap();
    let value_only: syn::File = syn::parse_str(
        "
        mod m { pub fn Shared() -> u32 { 2 } }
        use m::Shared;
        macro_rules! opaque { ($name:ident) => { 1u32 }; }
        fn f() -> u32 { opaque!(Shared) }
        ",
    )
    .unwrap();
    let type_context = SourceContext::new([(&[][..], &type_only)]);
    let value_context = SourceContext::new([(&[][..], &value_only)]);
    assert_eq!(
        function_form(&type_context, function(&type_only, "f")),
        function_form(&value_context, function(&value_only, "f"))
    );
}

#[test]
fn opaque_shared_qualified_names_retain_each_visible_namespace() {
    let ambiguous: syn::File = syn::parse_str(
        "
        mod m {
            pub type Shared = u32;
            pub fn Shared() -> u32 { 1 }
        }
        use m::Shared;
        macro_rules! call { ($value:ident) => { $value() }; }
        fn f() -> u32 { call!(Shared) }
        ",
    )
    .unwrap();
    let value_only: syn::File = syn::parse_str(
        "
        mod m { pub fn Shared() -> u32 { 1 } }
        use m::Shared;
        macro_rules! call { ($value:ident) => { $value() }; }
        fn f() -> u32 { call!(Shared) }
        ",
    )
    .unwrap();
    let ambiguous_context = SourceContext::new([(&[][..], &ambiguous)]);
    let value_context = SourceContext::new([(&[][..], &value_only)]);
    assert_ne!(
        function_form(&ambiguous_context, function(&ambiguous, "f")),
        function_form(&value_context, function(&value_only, "f"))
    );
}

#[test]
fn tuple_constructor_parameter_aliases_keep_their_value_reference_role() {
    let source: syn::File = syn::parse_str(
        "
        mod local { pub struct Wrap(pub u32); }
        mod a {
            use crate::local::Wrap as Left;
            fn first(Left(value): Left) -> u32 { value }
        }
        mod b {
            use crate::local::Wrap as Right;
            fn renamed(Right(item): Right) -> u32 { item }
        }
        mod c {
            use crate::local::Wrap as Other;
            fn different(Other(item): Other) -> u32 { item + 1 }
        }
        ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let first = module_function(&context, &source, "first");
    assert_eq!(first, module_function(&context, &source, "renamed"));
    assert_ne!(first, module_function(&context, &source, "different"));
}

#[test]
fn named_struct_imports_match_their_type_only_declarations() {
    let source: syn::File = syn::parse_str(
        "
        #[macro_export]
        macro_rules! size { ($ty:ty) => { std::mem::size_of::<$ty>() }; }
        mod local {
            pub struct Shared { pub value: u32 }
            pub struct Other { pub left: u32, pub right: u32 }
            fn direct() -> usize { crate::size!(Shared) }
        }
        mod a {
            use crate::local::Shared as Alias;
            fn imported() -> usize { crate::size!(Alias) }
        }
        mod b {
            use crate::local::Other as Alias;
            fn different() -> usize { crate::size!(Alias) }
        }
        ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    let direct = module_function(&context, &source, "direct");
    assert_eq!(direct, module_function(&context, &source, "imported"));
    assert_ne!(direct, module_function(&context, &source, "different"));
}

#[test]
fn seeded_self_ids_do_not_capture_free_helpers() {
    let source: syn::File = syn::parse_str(
        "
        fn zero() -> u32 { 1 }
        fn one() -> u32 { 2 }
        struct Owner { _field: () }
        impl Owner {
            fn first() -> u32 { zero() }
            fn second() -> u32 { one() }
            fn last() -> u32 { free_second() }
        }
        fn free_first() -> u32 { zero() }
        fn free_last() -> u32 { free_second() }
        fn free_second() -> u32 { one() }
        ",
    )
    .unwrap();
    let context = SourceContext::new([(&[][..], &source)]);
    for (free, method) in [
        ("free_first", "first"),
        ("free_second", "second"),
        ("free_last", "last"),
    ] {
        assert_eq!(
            function_form(&context, function(&source, free)),
            impl_method(&context, &source, method)
        );
    }
}
