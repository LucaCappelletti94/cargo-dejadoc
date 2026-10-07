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
