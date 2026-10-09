//! Original declaration and inherited reference identities.

use syn_canon::{CanonicalForm, SourceContext};

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

fn impl_forms<'a>(context: &'a SourceContext, file: &'a syn::File) -> Vec<CanonicalForm> {
    file.items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Impl(impl_item) => Some(&impl_item.items),
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
        .collect()
}

#[test]
fn marked_ident_owners_retain_their_seed_binding() {
    let source: syn::File = syn::parse_str(
        "
        struct A\u{301};
        impl A\u{301} {
            fn implicit() -> Self { Self }
            fn explicit() -> A\u{301} { A\u{301} }
        }
    ",
    )
    .unwrap();
    let context = SourceContext::new(core::iter::once((&[][..], &source)));
    let forms = impl_forms(&context, &source);
    assert_eq!(forms[0], forms[1]);
}

#[test]
fn owner_literal_payloads_keep_distinct_owners_distinct() {
    let source: syn::File = syn::parse_str(
        "
        struct Owner<const S: &'static str>;
        impl Owner<\" x\"> { fn f() -> Self { Self } }
        impl Owner<\"x\"> { fn f() -> Self { Self } }
    ",
    )
    .unwrap();
    let context = SourceContext::new(core::iter::once((&[][..], &source)));
    let forms = impl_forms(&context, &source);
    assert_ne!(forms[0], forms[1]);
}

#[test]
fn self_type_views_retain_their_original_owner_binding() {
    let local: syn::File =
        syn::parse_str("struct S { v: u32 } impl S { fn f(s: Self) -> Self { s } }").unwrap();
    let imported: syn::File = syn::parse_str(
        "mod m { pub struct S { v: u32 } } use m::S; impl S { fn f(s: Self) -> Self { s } }",
    )
    .unwrap();
    let local_context = SourceContext::new(core::iter::once((&[][..], &local)));
    let imported_context = SourceContext::new(core::iter::once((&[][..], &imported)));
    assert_ne!(
        impl_forms(&local_context, &local)[0],
        impl_forms(&imported_context, &imported)[0],
    );
}

#[test]
fn underscored_self_types_keep_their_seed_binding() {
    let local: syn::File =
        syn::parse_str("struct a_b { v: u32 } impl a_b { fn f(s: Self) -> Self { s } }").unwrap();
    let imported: syn::File = syn::parse_str(
        "mod m { pub struct a_b { v: u32 } } use m::a_b; impl a_b { fn f(s: Self) -> Self { s } }",
    )
    .unwrap();
    let local_context = SourceContext::new(core::iter::once((&[][..], &local)));
    let imported_context = SourceContext::new(core::iter::once((&[][..], &imported)));
    assert_ne!(
        impl_forms(&local_context, &local)[0],
        impl_forms(&imported_context, &imported)[0],
    );
}

#[test]
fn unresolved_self_types_keep_their_injective_owner_text() {
    let left: syn::File =
        syn::parse_str("struct W<T>(T); impl<T> W<T> { fn f(s: Self) -> Self { s } }").unwrap();
    let right: syn::File =
        syn::parse_str("struct X<T>(T); impl<T> X<T> { fn f(s: Self) -> Self { s } }").unwrap();
    let left_context = SourceContext::new(core::iter::once((&[][..], &left)));
    let right_context = SourceContext::new(core::iter::once((&[][..], &right)));
    assert_ne!(
        impl_forms(&left_context, &left)[0],
        impl_forms(&right_context, &right)[0],
    );
}

#[test]
fn dyn_trait_owners_keep_their_boundary_from_nominal_types() {
    let source: syn::File = syn::parse_str(
        "
        trait _Trait {}
        struct dyn_Trait;
        impl dyn _Trait { fn f(&self) -> *const Self { self } }
        impl dyn_Trait { fn f(&self) -> *const Self { self } }
    ",
    )
    .unwrap();
    let context = SourceContext::new(core::iter::once((&[][..], &source)));
    let forms = impl_forms(&context, &source);
    assert_ne!(forms[0], forms[1]);
}

#[test]
fn absolute_paths_keep_their_root_under_local_shadows() {
    let absolute: syn::File = syn::parse_str(
        "mod std { pub mod mem { pub fn drop(_: u32) {} } } fn f() { ::std::mem::drop(1u32); }",
    )
    .unwrap();
    let relative: syn::File = syn::parse_str(
        "mod std { pub mod mem { pub fn drop(_: u32) {} } } fn f() { std::mem::drop(1u32); }",
    )
    .unwrap();
    let absolute_context = SourceContext::new(core::iter::once((&[][..], &absolute)));
    let relative_context = SourceContext::new(core::iter::once((&[][..], &relative)));
    let absolute_fn = function(&absolute, "f");
    let relative_fn = function(&relative, "f");
    assert_ne!(
        absolute_context
            .function(&absolute_fn.sig, &absolute_fn.block)
            .unwrap()
            .canonicalize(),
        relative_context
            .function(&relative_fn.sig, &relative_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn failing_paths_keep_their_root_under_generic_shadows() {
    let absolute = syn::parse_str("fn f<std>() -> u32 { ::std::primitive::u32::MAX }").unwrap();
    let relative = syn::parse_str("fn f<std>() -> u32 { std::primitive::u32::MAX }").unwrap();
    assert_ne!(
        syn_canon::canonicalize_failing(absolute),
        syn_canon::canonicalize_failing(relative),
    );
}

#[test]
fn module_items_retain_their_seed_binding() {
    let pairs = [
        (
            "fn drop(_: u32) {} fn f(input: u32) { drop(input) }",
            "fn f(input: u32) { drop(input) }",
        ),
        (
            "const None: Option<u32> = Some(1); fn f() -> Option<u32> { None }",
            "fn f() -> Option<u32> { None }",
        ),
        (
            "static None: Option<u32> = Some(1); fn f() -> Option<u32> { None }",
            "fn f() -> Option<u32> { None }",
        ),
        ("struct String; fn f(_: String) {}", "fn f(_: String) {}"),
        (
            "union String { value: u32 } fn f(_: String) {}",
            "fn f(_: String) {}",
        ),
        (
            "enum Option<T> { Some(T), None } fn f(_: Option<u32>) {}",
            "fn f(_: Option<u32>) {}",
        ),
        ("trait Clone {} fn f<T: Clone>() {}", "fn f<T: Clone>() {}"),
        (
            "trait Sized = Copy; fn f<T: Sized>() {}",
            "fn f<T: Sized>() {}",
        ),
        (
            "mod std { pub mod mem { pub fn drop(_: u32) {} } } fn f(input: u32) { std::mem::drop(input) }",
            "fn f(input: u32) { std::mem::drop(input) }",
        ),
        (
            "macro_rules! println { () => {} } fn f() { println!(); }",
            "fn f() { println!(); }",
        ),
    ];
    for (defined, undefined) in pairs {
        let defined: syn::File = syn::parse_str(defined).unwrap();
        let undefined: syn::File = syn::parse_str(undefined).unwrap();
        let defined_context = SourceContext::new(core::iter::once((&[][..], &defined)));
        let undefined_context = SourceContext::new(core::iter::once((&[][..], &undefined)));
        let defined_fn = function(&defined, "f");
        let undefined_fn = function(&undefined, "f");
        assert_ne!(
            defined_context
                .function(&defined_fn.sig, &defined_fn.block)
                .unwrap()
                .canonicalize(),
            undefined_context
                .function(&undefined_fn.sig, &undefined_fn.block)
                .unwrap()
                .canonicalize(),
        );
    }
}

#[test]
fn imported_helpers_retain_their_kind_binding() {
    let pairs = [
        (
            "mod m { pub fn g() -> u32 { 1u32 } } use crate::m::g; fn f() -> u32 { g() }",
            "fn g() -> u32 { 1u32 } fn f() -> u32 { g() }",
        ),
        (
            "mod m { pub const G: u32 = 1; } use crate::m::G; fn f() -> u32 { G }",
            "const G: u32 = 1; fn f() -> u32 { G }",
        ),
        (
            "mod m { pub static G: u32 = 1; } use crate::m::G; fn f() -> u32 { G }",
            "static G: u32 = 1; fn f() -> u32 { G }",
        ),
    ];
    for (imported, local) in pairs {
        let imported: syn::File = syn::parse_str(imported).unwrap();
        let local: syn::File = syn::parse_str(local).unwrap();
        let imported_context = SourceContext::new(core::iter::once((&[][..], &imported)));
        let local_context = SourceContext::new(core::iter::once((&[][..], &local)));
        let imported_fn = function(&imported, "f");
        let local_fn = function(&local, "f");
        assert_ne!(
            imported_context
                .function(&imported_fn.sig, &imported_fn.block)
                .unwrap()
                .canonicalize(),
            local_context
                .function(&local_fn.sig, &local_fn.block)
                .unwrap()
                .canonicalize(),
        );
    }
}

#[test]
fn imported_helpers_keep_their_target_spelling() {
    let fn_mod = "mod m { pub fn g() -> u32 { 1u32 } pub fn h() -> u32 { 1u32 } pub fn j() -> u32 { 1u32 } }";
    let fn_pairs = [(
        format!("{fn_mod} use crate::m::h; fn f() -> u32 {{ h() }}"),
        format!("{fn_mod} use crate::m::j; fn f() -> u32 {{ j() }}"),
    )];
    let const_mod = "mod m { pub const G: u32 = 1; pub const H: u32 = 1; pub const J: u32 = 1; }";
    let const_pairs = [(
        format!("{const_mod} use crate::m::H; fn f() -> u32 {{ H }}"),
        format!("{const_mod} use crate::m::J; fn f() -> u32 {{ J }}"),
    )];
    let static_mod =
        "mod m { pub static G: u32 = 1; pub static H: u32 = 1; pub static J: u32 = 1; }";
    let static_pairs = [(
        format!("{static_mod} use crate::m::H; fn f() -> u32 {{ H }}"),
        format!("{static_mod} use crate::m::J; fn f() -> u32 {{ J }}"),
    )];
    for (left, right) in fn_pairs.into_iter().chain(const_pairs).chain(static_pairs) {
        let left: syn::File = syn::parse_str(&left).unwrap();
        let right: syn::File = syn::parse_str(&right).unwrap();
        let left_context = SourceContext::new(core::iter::once((&[][..], &left)));
        let right_context = SourceContext::new(core::iter::once((&[][..], &right)));
        let left_fn = function(&left, "f");
        let right_fn = function(&right, "f");
        assert_ne!(
            left_context
                .function(&left_fn.sig, &left_fn.block)
                .unwrap()
                .canonicalize(),
            right_context
                .function(&right_fn.sig, &right_fn.block)
                .unwrap()
                .canonicalize(),
        );
    }
}

#[test]
fn imported_helpers_differ_from_prelude_names() {
    let pairs = [
        (
            "mod m { pub fn drop(_: u32) {} } use crate::m::drop; fn f(input: u32) { drop(input) }",
            "fn f(input: u32) { drop(input) }",
        ),
        (
            "mod m { pub const None: Option<u32> = Some(1); } use crate::m::None; fn f() -> Option<u32> { None }",
            "fn f() -> Option<u32> { None }",
        ),
        (
            "mod m { pub static None: Option<u32> = Some(1); } use crate::m::None; fn f() -> Option<u32> { None }",
            "fn f() -> Option<u32> { None }",
        ),
    ];
    for (imported, undefined) in pairs {
        let imported: syn::File = syn::parse_str(imported).unwrap();
        let undefined: syn::File = syn::parse_str(undefined).unwrap();
        let imported_context = SourceContext::new(core::iter::once((&[][..], &imported)));
        let undefined_context = SourceContext::new(core::iter::once((&[][..], &undefined)));
        let imported_fn = function(&imported, "f");
        let undefined_fn = function(&undefined, "f");
        assert_ne!(
            imported_context
                .function(&imported_fn.sig, &imported_fn.block)
                .unwrap()
                .canonicalize(),
            undefined_context
                .function(&undefined_fn.sig, &undefined_fn.block)
                .unwrap()
                .canonicalize(),
        );
    }
}

#[test]
fn crate_and_self_imports_resolve_to_the_same_item() {
    let crated: syn::File = syn::parse_str(
        "mod m { pub struct T { pub v: u32 } } use crate::m::T; fn f(t: T) -> u32 { t.v }",
    )
    .unwrap();
    let selfed: syn::File = syn::parse_str(
        "mod m { pub struct T { pub v: u32 } } use self::m::T; fn f(t: T) -> u32 { t.v }",
    )
    .unwrap();
    let crated_context = SourceContext::new(core::iter::once((&[][..], &crated)));
    let selfed_context = SourceContext::new(core::iter::once((&[][..], &selfed)));
    let crated_fn = function(&crated, "f");
    let selfed_fn = function(&selfed, "f");
    assert_eq!(
        crated_context
            .function(&crated_fn.sig, &crated_fn.block)
            .unwrap()
            .canonicalize(),
        selfed_context
            .function(&selfed_fn.sig, &selfed_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn external_import_canons_keep_their_path_depth() {
    let path = ["external".to_owned(), "a".to_owned(), "b".to_owned()];
    let keyed: syn::File = syn::parse_str("struct b; fn f(x: b) -> b { x }").unwrap();
    let imported: syn::File = syn::parse_str("use external::a::b; fn f(x: b) -> b { x }").unwrap();
    let keyed_context = SourceContext::new(core::iter::once((&path[..], &keyed)));
    let imported_context = SourceContext::new(core::iter::once((&[][..], &imported)));
    let keyed_fn = function(&keyed, "f");
    let imported_fn = function(&imported, "f");
    assert_ne!(
        keyed_context
            .function(&keyed_fn.sig, &keyed_fn.block)
            .unwrap()
            .canonicalize(),
        imported_context
            .function(&imported_fn.sig, &imported_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn seed_binding_ids_keep_their_collision_free_identity() {
    let source: syn::File = syn::parse_str(
        "
        struct First;
        struct Target;
        struct Owner<T>(T);
        impl<T> Owner<T> {
            fn concrete(_: Target) {}
            fn generic(_: T) {}
        }
    ",
    )
    .unwrap();
    let context = SourceContext::new(core::iter::once((&[][..], &source)));
    let forms = impl_forms(&context, &source);
    assert_ne!(forms[0], forms[1]);
}

#[test]
fn generic_binding_ids_keep_their_collision_free_identity() {
    let original: syn::File = syn::parse_str(
        "trait Make { fn new(); } struct a; impl a { fn new() {} } fn g<T: Make>() { a::new(); }",
    )
    .unwrap();
    let renamed: syn::File = syn::parse_str(
        "trait Make { fn new(); } struct a; impl a { fn new() {} } fn g<T: Make>() { T::new(); }",
    )
    .unwrap();
    let original_context = SourceContext::new(core::iter::once((&[][..], &original)));
    let renamed_context = SourceContext::new(core::iter::once((&[][..], &renamed)));
    let original_fn = function(&original, "g");
    let renamed_fn = function(&renamed, "g");
    assert_ne!(
        original_context
            .function(&original_fn.sig, &original_fn.block)
            .unwrap()
            .canonicalize(),
        renamed_context
            .function(&renamed_fn.sig, &renamed_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn seed_item_ids_keep_their_collision_free_identity() {
    let original: syn::File = syn::parse_str(
        "struct X { v: u32 } struct Y { v: u32 } fn f(a: X, b: Y) -> u32 { a.v + b.v }",
    )
    .unwrap();
    let renamed: syn::File = syn::parse_str(
        "struct X { v: u32 } struct Z { v: u32 } fn f(a: X, b: Z) -> u32 { a.v + b.v }",
    )
    .unwrap();
    let original_context = SourceContext::new(core::iter::once((&[][..], &original)));
    let renamed_context = SourceContext::new(core::iter::once((&[][..], &renamed)));
    let original_fn = function(&original, "f");
    let renamed_fn = function(&renamed, "f");
    assert_ne!(
        original_context
            .function(&original_fn.sig, &original_fn.block)
            .unwrap()
            .canonicalize(),
        renamed_context
            .function(&renamed_fn.sig, &renamed_fn.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn type_generic_ids_keep_their_collision_free_identity() {
    let original: syn::File = syn::parse_str(
        "struct W<T, U>(T, U); struct X { v: u32 }
         impl<T, U> W<T, U> { fn f(x: X) -> u32 { x.v } }",
    )
    .unwrap();
    let renamed: syn::File = syn::parse_str(
        "struct W<T, U>(T, U); struct Y { v: u32 }
         impl<T, U> W<T, U> { fn f(x: Y) -> u32 { x.v } }",
    )
    .unwrap();
    let original_context = SourceContext::new(core::iter::once((&[][..], &original)));
    let renamed_context = SourceContext::new(core::iter::once((&[][..], &renamed)));
    let original_fn = &impl_forms(&original_context, &original)[0];
    let renamed_fn = &impl_forms(&renamed_context, &renamed)[0];
    assert_ne!(original_fn, renamed_fn,);
}

#[test]
fn type_generic_ids_keep_their_successor_identity() {
    let original: syn::File = syn::parse_str(
        "struct W<T, U>(T, U); fn take<T>() -> u32 { 0u32 }
         impl<T, U> W<T, U> { fn f(t: T, u: U) -> u32 { take::<U>() } }",
    )
    .unwrap();
    let renamed: syn::File = syn::parse_str(
        "struct W<T, U>(T, U); fn take<T>() -> u32 { 0u32 }
         impl<T, U> W<T, U> { fn f(t: T, u: U) -> u32 { take::<T>() } }",
    )
    .unwrap();
    let original_context = SourceContext::new(core::iter::once((&[][..], &original)));
    let renamed_context = SourceContext::new(core::iter::once((&[][..], &renamed)));
    let original_fn = &impl_forms(&original_context, &original)[0];
    let renamed_fn = &impl_forms(&renamed_context, &renamed)[0];
    assert_ne!(original_fn, renamed_fn,);
}

#[test]
fn const_generic_ids_keep_their_collision_free_identity() {
    let original: syn::File = syn::parse_str(
        "struct W<const N: u32, const M: u32>; struct X { v: u32 }
         impl<const N: u32, const M: u32> W<N, M> { fn f(x: X) -> u32 { x.v } }",
    )
    .unwrap();
    let renamed: syn::File = syn::parse_str(
        "struct W<const N: u32, const M: u32>; struct Y { v: u32 }
         impl<const N: u32, const M: u32> W<N, M> { fn f(x: Y) -> u32 { x.v } }",
    )
    .unwrap();
    let original_context = SourceContext::new(core::iter::once((&[][..], &original)));
    let renamed_context = SourceContext::new(core::iter::once((&[][..], &renamed)));
    let original_fn = &impl_forms(&original_context, &original)[0];
    let renamed_fn = &impl_forms(&renamed_context, &renamed)[0];
    assert_ne!(original_fn, renamed_fn,);
}

#[test]
fn const_generic_ids_keep_their_successor_identity() {
    let original: syn::File = syn::parse_str(
        "struct W<const N: u32, const M: u32>; fn take<T>() -> u32 { 0u32 }
         impl<const N: u32, const M: u32> W<N, M> { fn f() -> u32 { take::<W<N, M>>() } }",
    )
    .unwrap();
    let renamed: syn::File = syn::parse_str(
        "struct W<const N: u32, const M: u32>; fn take<T>() -> u32 { 0u32 }
         impl<const N: u32, const M: u32> W<N, M> { fn f() -> u32 { take::<W<M, N>>() } }",
    )
    .unwrap();
    let original_context = SourceContext::new(core::iter::once((&[][..], &original)));
    let renamed_context = SourceContext::new(core::iter::once((&[][..], &renamed)));
    let original_fn = &impl_forms(&original_context, &original)[0];
    let renamed_fn = &impl_forms(&renamed_context, &renamed)[0];
    assert_ne!(original_fn, renamed_fn,);
}

#[test]
fn imported_primitive_facts_select_the_named_alias() {
    let form = |width: &str, alias: &str, parameter: &str| {
        let source: syn::File = syn::parse_str(&format!(
            "mod types {{ pub type Unrelated = u8; pub type Word = {width}; }}
             use types::Word as {alias};
             fn value({parameter}: {alias}) -> {alias} {{ {parameter} }}"
        ))
        .unwrap();
        let context = SourceContext::new(core::iter::once((&[][..], &source)));
        function_form(&context, function(&source, "value"))
    };
    let original = form("u32", "Word", "input");
    assert_eq!(original, form("u32", "Scalar", "argument"));
    assert_ne!(original, form("u64", "Word", "input"));
}

#[test]
fn impl_observers_preserve_literal_declaration_order() {
    for attribute in ["", "#[observer::inspect]"] {
        let source: syn::File = syn::parse_str(&format!(
            "struct Bag;
             {attribute}
             impl Bag {{
                 fn original() -> (u32, u32) {{
                     let low = 3u32; let high = 5u32; (low, high)
                 }}
                 fn reordered() -> (u32, u32) {{
                     let high = 5u32; let low = 3u32; (low, high)
                 }}
                 fn renamed() -> (u32, u32) {{
                     let first = 3u32; let second = 5u32; (first, second)
                 }}
             }}"
        ))
        .unwrap();
        let context = SourceContext::new(core::iter::once((&[][..], &source)));
        let forms = impl_forms(&context, &source);
        assert_eq!(forms[0], forms[2]);
        if attribute.is_empty() {
            assert_eq!(forms[0], forms[1]);
        } else {
            assert_ne!(forms[0], forms[1]);
        }
    }
}

#[test]
fn trait_observers_preserve_literal_declaration_order() {
    for attribute in ["", "#[observer::inspect]"] {
        let source: syn::File = syn::parse_str(&format!(
            "{attribute}
             trait Bag {{
                 fn original() -> (u32, u32) {{
                     let low = 3u32; let high = 5u32; (low, high)
                 }}
                 fn reordered() -> (u32, u32) {{
                     let high = 5u32; let low = 3u32; (low, high)
                 }}
                 fn renamed() -> (u32, u32) {{
                     let first = 3u32; let second = 5u32; (first, second)
                 }}
             }}"
        ))
        .unwrap();
        let context = SourceContext::new(core::iter::once((&[][..], &source)));
        let syn::Item::Trait(item) = &source.items[0] else {
            unreachable!()
        };
        let forms: Vec<_> = item
            .items
            .iter()
            .filter_map(|item| match item {
                syn::TraitItem::Fn(method) => Some(
                    context
                        .function(&method.sig, method.default.as_ref().unwrap())
                        .unwrap()
                        .canonicalize(),
                ),
                _ => None,
            })
            .collect();
        assert_eq!(forms[0], forms[2]);
        if attribute.is_empty() {
            assert_eq!(forms[0], forms[1]);
        } else {
            assert_ne!(forms[0], forms[1]);
        }
    }
}

#[test]
fn owner_observers_keep_methods_distinct_from_unobserved_bodies() {
    for owner in ["impl Bag", "trait Bag", "mod bag"] {
        for (result, initializer) in [
            ("u8", "3u8"),
            ("u16", "3u16"),
            ("u32", "3u32"),
            ("u64", "3u64"),
            ("u128", "3u128"),
            ("i8", "3i8"),
            ("i16", "3i16"),
            ("i32", "3i32"),
            ("i64", "3i64"),
            ("i128", "3i128"),
            ("bool", "true"),
            ("u32", "3u32 << 1"),
            ("u32", "3u32 >> 1"),
            ("u32", "3u32 & 1u32"),
            ("u32", "3u32 | 1u32"),
            ("u32", "3u32 ^ 1u32"),
            ("u32", "3u32 & 1"),
            ("u32", "3 & 1u32"),
            ("bool", "true & false"),
            ("bool", "true | false"),
            ("bool", "true ^ false"),
        ] {
            let form = |attribute: &str, binding: &str| {
                let prefix = if owner.starts_with("impl") {
                    "struct Bag;"
                } else {
                    ""
                };
                let source: syn::File = syn::parse_str(&format!(
                    "{prefix} {attribute} {owner} {{
                         fn value() -> {result} {{
                             let {binding} = {initializer}; {binding}
                         }}
                     }}"
                ))
                .unwrap();
                let context = SourceContext::new(core::iter::once((&[][..], &source)));
                if owner.starts_with("impl") {
                    impl_forms(&context, &source).remove(0)
                } else if owner.starts_with("trait") {
                    let syn::Item::Trait(item) = &source.items[0] else {
                        unreachable!()
                    };
                    let syn::TraitItem::Fn(method) = &item.items[0] else {
                        unreachable!()
                    };
                    context
                        .function(&method.sig, method.default.as_ref().unwrap())
                        .unwrap()
                        .canonicalize()
                } else {
                    let syn::Item::Mod(item) = &source.items[0] else {
                        unreachable!()
                    };
                    let syn::Item::Fn(function) = &item.content.as_ref().unwrap().1[0] else {
                        unreachable!()
                    };
                    context
                        .function(&function.sig, &function.block)
                        .unwrap()
                        .canonicalize()
                }
            };
            let plain = form("", "value");
            assert_eq!(plain, form("#[allow(dead_code)]", "renamed"));
            let observed = form("#[observer::inspect]", "value");
            assert_eq!(observed, form("#[observer::inspect]", "renamed"));
            assert_ne!(plain, observed, "{owner} {initializer}");
        }
    }
}

#[test]
fn owner_observers_keep_standard_macro_bodies_distinct() {
    for body in [
        "std::print!(\"{}\", 3u32);",
        "std::println!(\"{}\", 3u32);",
        "std::eprint!(\"{}\", 3u32);",
        "std::eprintln!(\"{}\", 3u32);",
        "std::format!(\"{}\", 3u32);",
        "std::format_args!(\"{}\", 3u32);",
        "std::assert!(true);",
        "std::assert_eq!(3u32, 3u32);",
        "std::assert_ne!(3u32, 5u32);",
        "std::debug_assert!(true);",
        "std::debug_assert_eq!(3u32, 3u32);",
        "std::debug_assert_ne!(3u32, 5u32);",
        "std::panic!(\"stop\");",
        "std::todo!(\"stop\");",
        "std::unimplemented!(\"stop\");",
        "std::unreachable!(\"stop\");",
    ] {
        let form = |attribute: &str| {
            let source: syn::File = syn::parse_str(&format!(
                "struct Bag; {attribute} impl Bag {{ fn value() {{ {body} }} }}"
            ))
            .unwrap();
            let context = SourceContext::new(core::iter::once((&[][..], &source)));
            impl_forms(&context, &source).remove(0)
        };
        let plain = form("");
        assert_eq!(plain, form("#[allow(dead_code)]"));
        assert_ne!(plain, form("#[observer::inspect]"), "{body}");
    }
}

#[test]
fn anonymous_type_expansions_block_builtin_primitive_assumptions() {
    let form = |expansion: &str, parameter: &str| {
        let source: syn::File = syn::parse_str(&format!(
            "macro_rules! declare_width {{ () => {{
                 #[allow(non_camel_case_types)] type u32 = u64;
             }}; }}
             {expansion}
             fn value({parameter}: u32) -> u32 {{ {parameter} }}"
        ))
        .unwrap();
        let context = SourceContext::new(core::iter::once((&[][..], &source)));
        function_form(&context, function(&source, "value"))
    };
    let original = form("", "input");
    assert_eq!(original, form("", "argument"));
    let expanded = form("declare_width!();", "input");
    assert_eq!(expanded, form("declare_width!();", "argument"));
    assert_ne!(original, expanded);
}

#[test]
fn owner_observers_keep_total_tails_distinct() {
    for (result, tail) in [
        ("u32", "3u32"),
        ("bool", "true"),
        ("u32", "3u32 << 1"),
        ("u32", "3u32 & 1"),
        ("(u32, u64)", "(3u32, 5u64)"),
    ] {
        let form = |attribute: &str| {
            let source: syn::File = syn::parse_str(&format!(
                "struct Bag; {attribute} impl Bag {{ fn value() -> {result} {{ {tail} }} }}"
            ))
            .unwrap();
            let context = SourceContext::new(core::iter::once((&[][..], &source)));
            impl_forms(&context, &source).remove(0)
        };
        let plain = form("");
        assert_eq!(plain, form("#[allow(dead_code)]"));
        assert_ne!(plain, form("#[observer::inspect]"), "{tail}");
    }
}

#[test]
fn owner_observers_keep_transparent_scalar_groups_distinct() {
    let form = |attribute: &str| {
        let mut source: syn::File = syn::parse_str(&format!(
            "struct Bag; {attribute} impl Bag {{ fn value() -> u32 {{ 3u32 }} }}"
        ))
        .unwrap();
        let syn::Item::Impl(item) = &mut source.items[1] else {
            unreachable!()
        };
        let syn::ImplItem::Fn(method) = &mut item.items[0] else {
            unreachable!()
        };
        let syn::Stmt::Expr(expression, None) = &mut method.block.stmts[0] else {
            unreachable!()
        };
        *expression = syn::Expr::Group(syn::ExprGroup {
            attrs: Vec::new(),
            group_token: syn::token::Group::default(),
            expr: Box::new(expression.clone()),
        });
        let context = SourceContext::new(core::iter::once((&[][..], &source)));
        impl_forms(&context, &source).remove(0)
    };
    let plain = form("");
    assert_eq!(plain, form("#[allow(dead_code)]"));
    assert_ne!(plain, form("#[observer::inspect]"));
}

#[test]
fn owner_observers_keep_item_shaped_observations_distinct() {
    let form = |attribute: &str| {
        let mut source: syn::File = syn::parse_str(&format!(
            "struct Bag; {attribute} impl Bag {{ fn value() {{ std::println!(\"{{}}\", 3u32); }} }}"
        ))
        .unwrap();
        let syn::Item::Impl(item) = &mut source.items[1] else {
            unreachable!()
        };
        let syn::ImplItem::Fn(method) = &mut item.items[0] else {
            unreachable!()
        };
        let syn::Stmt::Macro(statement) = &method.block.stmts[0] else {
            unreachable!()
        };
        method.block.stmts[0] = syn::Stmt::Item(syn::Item::Macro(syn::ItemMacro {
            attrs: statement.attrs.clone(),
            ident: None,
            mac: statement.mac.clone(),
            semi_token: statement.semi_token,
        }));
        let context = SourceContext::new(core::iter::once((&[][..], &source)));
        impl_forms(&context, &source).remove(0)
    };
    let plain = form("");
    assert_eq!(plain, form("#[allow(dead_code)]"));
    assert_ne!(plain, form("#[observer::inspect]"));
}

#[test]
fn parameter_shadows_preserve_renamed_local_proofs() {
    let form = |body: &str| {
        let source: syn::File =
            syn::parse_str(&format!("fn value(input: u32) -> u32 {{ {body} }}")).unwrap();
        let context = SourceContext::new(core::iter::once((&[][..], &source)));
        function_form(&context, function(&source, "value"))
    };
    assert_eq!(
        form("let input = input; let copied = input; copied"),
        form("let local = input; let copied = local; copied"),
    );
    assert_eq!(
        form("let mut input = input; let copied = input; copied"),
        form("let mut local = input; let copied = local; copied"),
    );
}

#[test]
fn unrelated_bindings_preserve_renamed_producer_proofs() {
    let form = |binding: &str| {
        let source: syn::File = syn::parse_str(&format!(
            "fn value() -> u32 {{ let {binding} = 3u32; let other = (); {binding} }}"
        ))
        .unwrap();
        let context = SourceContext::new(core::iter::once((&[][..], &source)));
        function_form(&context, function(&source, "value"))
    };
    assert_eq!(form("value"), form("xyzzy"));
}

#[test]
fn nested_scalar_proofs_preserve_the_owner_observer_boundary() {
    let form = |attribute: &str, binding: &str| {
        let source: syn::File = syn::parse_str(&format!(
            "struct Bag; {attribute} impl Bag {{
                 fn value() -> u32 {{
                     {}let {binding} = 3u32; {binding}{}
                 }}
             }}",
            "{".repeat(63),
            "}".repeat(63),
        ))
        .unwrap();
        let context = SourceContext::new(core::iter::once((&[][..], &source)));
        impl_forms(&context, &source).remove(0)
    };
    let plain = form("", "value");
    assert_eq!(plain, form("", "renamed"));
    assert_eq!(plain, form("#[allow(dead_code)]", "value"));
    assert_ne!(plain, form("#[observer::inspect]", "value"));
}

#[test]
fn qualified_assignments_do_not_invalidate_same_named_local_proofs() {
    let form = |binding: &str| {
        let source: syn::File = syn::parse_str(&format!(
            "mod storage {{ pub static mut VALUE: u32 = 0; }}
             unsafe fn value() -> u32 {{
                 let {binding} = 3u32;
                 storage::VALUE = 5u32;
                 {binding}
             }}"
        ))
        .unwrap();
        let context = SourceContext::new(core::iter::once((&[][..], &source)));
        function_form(&context, function(&source, "value"))
    };
    assert_eq!(form("storage"), form("local"));
    assert_eq!(form("storage"), form("xyzzy"));
}

#[test]
fn borrowed_loop_patterns_preserve_shadowed_producer_identity() {
    for (declaration, input, pattern) in [
        ("", "&[u32]", "value"),
        ("", "&[(u32, u32)]", "(value, _)"),
        ("", "&[((u32, u32), u32)]", "((value, _), _)"),
        ("struct Record(u32);", "&[Record]", "Record(value)"),
        (
            "struct Record { field: u32 }",
            "&[Record]",
            "Record { field: value }",
        ),
        ("", "&[[u32; 2]]", "[value, ..]"),
        ("", "&[u32]", "&value"),
    ] {
        let form = |binding: &str| {
            let source: syn::File = syn::parse_str(&format!(
                "{declaration} fn f(input: {input}) -> u32 {{
                     let {binding} = 3u32;
                     for {pattern} in input {{ let _ = &value; }}
                     {binding}
                 }}"
            ))
            .unwrap();
            let context = SourceContext::new(core::iter::once((&[][..], &source)));
            function_form(&context, function(&source, "f"))
        };
        assert_eq!(form("value"), form("result"), "{pattern}");
        assert_eq!(form("value"), form("xyzzy"), "{pattern}");
    }
}

#[test]
fn borrowed_match_patterns_preserve_shadowed_producer_identity() {
    for pattern in [
        "Choice::First(value) if *value > 0",
        "Choice::First(value) | Choice::Second(value)",
    ] {
        let form = |binding: &str| {
            let source: syn::File = syn::parse_str(&format!(
                "enum Choice {{ First(u32), Second(u32) }}
                 fn f(input: &Choice) -> u32 {{
                     let {binding} = 3u32;
                     match input {{ {pattern} => {{ let _ = &value; }}, _ => {{}} }}
                     {binding}
                 }}"
            ))
            .unwrap();
            let context = SourceContext::new(core::iter::once((&[][..], &source)));
            function_form(&context, function(&source, "f"))
        };
        assert_eq!(form("value"), form("result"), "{pattern}");
        assert_eq!(form("value"), form("xyzzy"), "{pattern}");
    }
}

#[test]
fn borrowed_closure_patterns_preserve_shadowed_producer_identity() {
    for (pattern, argument) in [("value: u32", "7u32"), ("(value, _)", "(7u32, 9u32)")] {
        let form = |binding: &str| {
            let source: syn::File = syn::parse_str(&format!(
                "fn f() -> u32 {{
                     let {binding} = 3u32;
                     let calculate = |{pattern}| {{ let _ = &value; }};
                     calculate({argument});
                     {binding}
                 }}"
            ))
            .unwrap();
            let context = SourceContext::new(core::iter::once((&[][..], &source)));
            function_form(&context, function(&source, "f"))
        };
        assert_eq!(form("value"), form("result"), "{pattern}");
        assert_eq!(form("value"), form("xyzzy"), "{pattern}");
    }
}

#[test]
fn unproven_shadowing_locals_invalidate_the_previous_primitive_proof() {
    let form = |binding: &str| {
        let source: syn::File = syn::parse_str(&format!(
            "fn helper() -> u32 {{ 7u32 }}
             fn value() -> u32 {{
                 let {binding} = 3u32;
                 let value = helper();
                 value
             }}"
        ))
        .unwrap();
        let context = SourceContext::new(core::iter::once((&[][..], &source)));
        function_form(&context, function(&source, "value"))
    };
    assert_eq!(form("value"), form("unused"));
    assert_eq!(form("value"), form("xyzzy"));
}

#[test]
fn mixed_primitive_overloads_keep_their_unproven_owner_tails() {
    for (implementation, tail) in [
        (
            "impl std::ops::BitAnd<Scalar> for u32 {
                 type Output = u32;
                 fn bitand(self, rhs: Scalar) -> u32 {
                     std::println!(\"{self}\");
                     self & rhs.0
                 }
             }",
            "1u32 & argument",
        ),
        (
            "impl std::ops::BitAnd<u32> for Scalar {
                 type Output = u32;
                 fn bitand(self, rhs: u32) -> u32 {
                     std::println!(\"{rhs}\");
                     self.0 & rhs
                 }
             }",
            "argument & 1u32",
        ),
    ] {
        let form = |attribute: &str, body: &str, parameter: &str| {
            let source: syn::File = syn::parse_str(&format!(
                "struct Scalar(u32);
                 struct Bag;
                 impl Copy for Scalar {{}}
                 impl Clone for Scalar {{ fn clone(&self) -> Self {{ *self }} }}
                 {implementation}
                 {attribute} impl Bag {{
                     fn value({parameter}: Scalar) -> impl Copy {{ {body} }}
                 }}"
            ))
            .unwrap();
            let context = SourceContext::new(core::iter::once((&[][..], &source)));
            impl_forms(&context, &source).pop().unwrap()
        };
        let plain = form("", tail, "argument");
        assert_eq!(plain, form("#[allow(dead_code)]", tail, "argument"));
        assert_eq!(plain, form("#[observer::inspect]", tail, "argument"));
        assert_eq!(plain, form("", &tail.replace("argument", "input"), "input"));
        assert_ne!(
            form("", "3u32", "argument"),
            form("#[observer::inspect]", "3u32", "argument")
        );
    }
}

#[test]
fn overflowing_literal_shifts_keep_their_unproven_owner_tails() {
    let form = |attribute: &str, count: u32, owner: &str| {
        let source: syn::File = syn::parse_str(&format!(
            "struct {owner}; {attribute} impl {owner} {{
                 #[allow(arithmetic_overflow)]
                 fn value() -> impl Copy {{ 1u32 << {count}u32 }}
             }}"
        ))
        .unwrap();
        let context = SourceContext::new(core::iter::once((&[][..], &source)));
        impl_forms(&context, &source).pop().unwrap()
    };
    for count in [32, 33] {
        let plain = form("", count, "Bag");
        assert_eq!(plain, form("#[allow(dead_code)]", count, "Bag"));
        assert_eq!(plain, form("#[observer::inspect]", count, "Bag"));
        assert_eq!(plain, form("", count, "Parcel"));
    }
    assert_ne!(form("", 31, "Bag"), form("#[observer::inspect]", 31, "Bag"));
}

#[test]
fn primitive_parameter_tails_preserve_owner_proof_boundaries() {
    let form = |prefix: &str, ty: &str, attribute: &str, binding: &str, grouped: bool| {
        let mut source: syn::File = syn::parse_str(&format!(
            "{prefix} struct Bag; {attribute} impl Bag {{
                 fn value({binding}: {ty}) -> impl Copy {{ {binding} }}
             }}"
        ))
        .unwrap();
        if grouped {
            let syn::Item::Impl(item) = source.items.last_mut().unwrap() else {
                unreachable!()
            };
            let syn::ImplItem::Fn(function) = &mut item.items[0] else {
                unreachable!()
            };
            let syn::FnArg::Typed(parameter) = function.sig.inputs.first_mut().unwrap() else {
                unreachable!()
            };
            let elem = core::mem::replace(
                &mut *parameter.ty,
                syn::Type::Verbatim(proc_macro2::TokenStream::new()),
            );
            *parameter.ty = syn::Type::Group(syn::TypeGroup {
                attrs: Vec::new(),
                group_token: syn::token::Group::default(),
                elem: Box::new(elem),
            });
        }
        let context = SourceContext::new(core::iter::once((&[][..], &source)));
        impl_forms(&context, &source).pop().unwrap()
    };
    for (prefix, ty) in [
        ("", "u32"),
        ("", "bool"),
        ("type Width = u32;", "Width"),
        ("type Width = bool;", "Width"),
        ("type Width = u32; use Width as Input;", "Input"),
        ("", "::std::primitive::u32"),
    ] {
        for grouped in [false, true] {
            let plain = form(prefix, ty, "", "argument", grouped);
            assert_eq!(
                plain,
                form(prefix, ty, "#[allow(dead_code)]", "argument", grouped)
            );
            assert_eq!(plain, form(prefix, ty, "", "input", grouped));
            assert_ne!(
                plain,
                form(prefix, ty, "#[observer::inspect]", "argument", grouped)
            );
        }
    }
    let scalar = "struct Scalar(u32); impl Copy for Scalar {}
                  impl Clone for Scalar { fn clone(&self) -> Self { *self } }";
    assert_eq!(
        form(scalar, "Scalar", "", "argument", false),
        form(scalar, "Scalar", "#[observer::inspect]", "argument", false)
    );
}

#[test]
fn unmodeled_macro_calls_keep_owner_bodies_opaque() {
    let form = |prefix: &str, body: &str, attribute: &str, binding: &str| {
        let source: syn::File = syn::parse_str(&format!(
            "{prefix} struct Bag; {attribute} impl Bag {{
                 fn value() -> impl Copy {{ let {binding} = 3u32; {body} }}
             }}"
        ))
        .unwrap();
        let context = SourceContext::new(core::iter::once((&[][..], &source)));
        impl_forms(&context, &source).pop().unwrap()
    };
    let imported = "mod observers {
        macro_rules! capture { ($v:expr) => { $v } }
        pub(crate) use capture as print;
    }";
    for (prefix, body) in [
        ("extern crate observer;", "observer::passthrough!(value)"),
        (
            "macro_rules! inspect { ($v:expr) => { $v } }",
            "inspect!(value)",
        ),
        (
            "macro_rules! print { ($v:expr) => { $v } }",
            "print!(value)",
        ),
        (
            "mod std { macro_rules! capture { ($v:expr) => { $v } }
             pub(crate) use capture as print; }",
            "std::print!(value)",
        ),
        (
            "mod core { macro_rules! capture { ($v:expr) => { $v } }
             pub(crate) use capture as print; }",
            "core::print!(value)",
        ),
        (
            imported,
            "let result = print!(value); use observers::print; result",
        ),
    ] {
        let plain = form(prefix, body, "", "value");
        assert_eq!(plain, form(prefix, body, "#[allow(dead_code)]", "value"));
        assert_eq!(plain, form(prefix, body, "#[observer::inspect]", "value"));
        assert_eq!(
            plain,
            form(prefix, &body.replace("value", "input"), "", "input")
        );
    }
    assert_ne!(
        form("", "print!(\"\"); value", "", "value"),
        form("", "print!(\"\"); value", "#[observer::inspect]", "value")
    );
}
