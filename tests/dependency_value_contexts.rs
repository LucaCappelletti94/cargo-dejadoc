//! Public contextual views of the value folds' original sources.

#[path = "support/function.rs"]
mod functions;
#[path = "support/impl_item.rs"]
mod impl_item;

use functions::function;
use impl_item::impl_method;

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

fn module_function<'a>(file: &'a syn::File, module: &str, name: &str) -> &'a syn::ItemFn {
    file.items
        .iter()
        .find_map(|item| match item {
            syn::Item::Mod(mod_item) if mod_item.ident == module => {
                mod_item.content.as_ref().and_then(|(_, items)| {
                    items.iter().find_map(|inner| match inner {
                        syn::Item::Fn(function) if function.sig.ident == name => Some(function),
                        _ => None,
                    })
                })
            }
            _ => None,
        })
        .unwrap()
}

fn view<'a>(
    context: &syn_canon::SourceContext<'a>,
    sig: &'a syn::Signature,
    block: &'a syn::Block,
) -> syn_canon::CanonicalForm {
    context.function(sig, block).unwrap().canonicalize()
}

#[test]
fn value_function_views_preserve_contextual_relations() {
    let source: syn::File = syn::parse_str(
        "
        type Scalar = u32;
        fn aliased(input: Scalar) -> Scalar { let mask = input & 15u32; let copied = mask; copied ^ 3u32 }
        fn renamed(slot: Scalar) -> Scalar { let mask = slot & 15u32; mask ^ 3u32 }
        fn held(gate: Scalar) -> Scalar { let mask = gate & 15u32; let seen = &mask; mask ^ 3u32 }
        struct Owner;
        impl Owner {
            fn temporary(input: Scalar) -> Scalar { let mask = input & 15u32; mask ^ 3u32 }
            fn inlined(input: Scalar) -> Scalar { (input & 15u32) ^ 3u32 }
            fn observed(input: Scalar) -> Scalar { let mask = input & 15u32; let seen = &mask; *seen; mask ^ 3u32 }
        }
        ",
    )
    .unwrap();
    let context = syn_canon::SourceContext::new(core::iter::once((&[][..], &source)));
    let canonical = |name: &str| {
        let f = function(&source, name);
        view(&context, &f.sig, &f.block)
    };
    assert_eq!(canonical("aliased"), canonical("renamed"));
    assert_ne!(canonical("aliased"), canonical("held"));
    let method = |name: &str| {
        let member = impl_method(&source, name);
        view(&context, &member.sig, &member.block)
    };
    assert_eq!(method("temporary"), method("inlined"));
    assert_ne!(method("temporary"), method("observed"));
}

#[test]
fn raw_names_take_the_value_folds() {
    let raw = "fn f(r#input: u32) -> u32 { let r#mask = r#input & 15u32; r#mask ^ 3u32 }";
    let plain = "fn f(input: u32) -> u32 { let mask = input & 15u32; mask ^ 3u32 }";
    assert_eq!(form(raw), form(plain));
    let raw_held = "fn f(r#input: u32) -> u32 { let r#mask = r#input & 15u32; let seen = &r#mask; r#mask ^ 3u32 }";
    let plain_held =
        "fn f(input: u32) -> u32 { let mask = input & 15u32; let seen = &mask; mask ^ 3u32 }";
    assert_eq!(form(raw_held), form(plain_held));
    assert_eq!(
        syn_canon::canonicalize_failing(syn::parse_str(raw).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(plain).unwrap())
    );
}

struct SpanWiper;

impl syn::visit_mut::VisitMut for SpanWiper {
    fn visit_ident_mut(&mut self, ident: &mut syn::Ident) {
        ident.set_span(proc_macro2::Span::call_site());
    }

    fn visit_lit_mut(&mut self, lit: &mut syn::Lit) {
        lit.set_span(proc_macro2::Span::call_site());
    }
}

fn wiped_form(source: &str, compiles: bool) -> syn_canon::CanonicalForm {
    let mut file = syn::parse_str::<syn::File>(source).unwrap();
    syn::visit_mut::visit_file_mut(&mut SpanWiper, &mut file);
    if compiles {
        syn_canon::canonicalize(file)
    } else {
        syn_canon::canonicalize_failing(file)
    }
}

#[test]
fn constructed_equal_spans_preserve_value_relations() {
    let plain = "fn f(input: u32) -> u32 { let mask = input & 15u32; mask ^ 3u32 }";
    let folded = "fn g(input: u32) -> u32 { (input & 15u32) ^ 3u32 }";
    let held_a =
        "fn f(input: u32) -> u32 { let mask = input & 15u32; let seen = &mask; mask ^ 3u32 }";
    let held_b = "fn g(input: u32) -> u32 { let mask = input & 15u32; mask ^ 3u32 }";
    assert_eq!(wiped_form(plain, true), form(plain));
    assert_eq!(wiped_form(plain, true), wiped_form(folded, true));
    assert_ne!(wiped_form(plain, false), wiped_form(folded, false));
    assert_eq!(wiped_form(held_a, true), form(held_a));
    assert_ne!(wiped_form(held_a, true), wiped_form(held_b, true));
    let tuple =
        "fn f(input: u32) -> (u32, u32) { let (a, b) = (input & 15u32, input >> 4u32); (a, b) }";
    let decomposed =
        "fn g(input: u32) -> (u32, u32) { let a = input & 15u32; let b = input >> 4u32; (a, b) }";
    assert_eq!(wiped_form(tuple, true), wiped_form(decomposed, true));
    assert_ne!(wiped_form(tuple, false), wiped_form(decomposed, false));
}

#[test]
fn module_scoped_aliases_keep_their_value_relations() {
    let source: syn::File = syn::parse_str(
        "
        mod scoped {
            type Scalar = u32;
            pub fn aliased(input: Scalar) -> Scalar { let mask = input & 15u32; mask ^ 3u32 }
            pub fn inlined(input: Scalar) -> Scalar { (input & 15u32) ^ 3u32 }
        }
        mod bare {
            pub fn inlined(input: u32) -> u32 { (input & 15u32) ^ 3u32 }
        }
        mod signed {
            type Scalar = i32;
            pub fn inlined(input: Scalar) -> Scalar { (input & 15i32) ^ 3i32 }
        }
        ",
    )
    .unwrap();
    let context = syn_canon::SourceContext::new(core::iter::once((&[][..], &source)));
    let scoped = |name: &str| {
        let f = module_function(&source, "scoped", name);
        view(&context, &f.sig, &f.block)
    };
    let inlined = |module: &str| {
        let f = module_function(&source, module, "inlined");
        view(&context, &f.sig, &f.block)
    };
    assert_eq!(scoped("aliased"), scoped("inlined"));
    assert_ne!(scoped("inlined"), inlined("bare"));
    assert_ne!(scoped("inlined"), inlined("signed"));
}

#[test]
fn incomplete_glob_and_conditional_contexts_gate_the_value_folds() {
    let complete: syn::File = syn::parse_str(
        "
        mod helper { pub type Width = u32; }
        use helper::Width;
        fn aliased(input: Width) -> Width { let mask = input & 15u32; mask ^ 3u32 }
        fn inlined(input: Width) -> Width { (input & 15u32) ^ 3u32 }
        ",
    )
    .unwrap();
    let complete_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &complete)));
    let aliased = function(&complete, "aliased");
    let inlined = function(&complete, "inlined");
    assert_eq!(
        view(&complete_context, &aliased.sig, &aliased.block),
        view(&complete_context, &inlined.sig, &inlined.block)
    );
    let incomplete: syn::File = syn::parse_str(
        "
        mod helper;
        use helper::Width;
        fn aliased(input: Width) -> Width { let mask = input & 15u32; mask ^ 3u32 }
        fn inlined(input: Width) -> Width { (input & 15u32) ^ 3u32 }
        ",
    )
    .unwrap();
    let incomplete_context =
        syn_canon::SourceContext::new(core::iter::once((&[][..], &incomplete)));
    let aliased = function(&incomplete, "aliased");
    let inlined = function(&incomplete, "inlined");
    assert_ne!(
        view(&incomplete_context, &aliased.sig, &aliased.block),
        view(&incomplete_context, &inlined.sig, &inlined.block)
    );
    let globbed: syn::File = syn::parse_str(
        "
        mod plugin;
        use plugin::*;
        fn aliased(input: u32) -> u32 { let mask = input & 15u32; mask ^ 3u32 }
        fn inlined(input: u32) -> u32 { (input & 15u32) ^ 3u32 }
        ",
    )
    .unwrap();
    assert_ne!(
        form(
            "mod plugin; use plugin::*; fn f(input: u32) -> u32 { let mask = input & 15u32; mask ^ 3u32 }"
        ),
        form("mod plugin; use plugin::*; fn g(input: u32) -> u32 { (input & 15u32) ^ 3u32 }")
    );
    let globbed_context = syn_canon::SourceContext::new(core::iter::once((&[][..], &globbed)));
    let aliased = function(&globbed, "aliased");
    let inlined = function(&globbed, "inlined");
    assert_ne!(
        view(&globbed_context, &aliased.sig, &aliased.block),
        view(&globbed_context, &inlined.sig, &inlined.block)
    );
    let conditional: syn::File = syn::parse_str(
        "
        #[cfg(not(any()))] type Scalar = u32;
        #[cfg(any())] type Scalar = i32;
        fn aliased(input: Scalar) -> Scalar { let mask = input & 15u32; mask ^ 3u32 }
        fn inlined(input: Scalar) -> Scalar { (input & 15u32) ^ 3u32 }
        ",
    )
    .unwrap();
    let conditional_context =
        syn_canon::SourceContext::new(core::iter::once((&[][..], &conditional)));
    let aliased = function(&conditional, "aliased");
    let inlined = function(&conditional, "inlined");
    assert_ne!(
        view(&conditional_context, &aliased.sig, &aliased.block),
        view(&conditional_context, &inlined.sig, &inlined.block)
    );
}

#[test]
fn live_cfg_attributes_gate_the_value_folds_in_views() {
    let source: syn::File = syn::parse_str(
        "
        #[cfg(unix)]
        fn gated(input: u32) -> u32 { let mask = input & 15u32; mask ^ 3u32 }
        #[cfg(unix)]
        fn folded(input: u32) -> u32 { (input & 15u32) ^ 3u32 }
        fn plain(input: u32) -> u32 { let mask = input & 15u32; mask ^ 3u32 }
        fn bare(input: u32) -> u32 { (input & 15u32) ^ 3u32 }
        ",
    )
    .unwrap();
    let context = syn_canon::SourceContext::new(core::iter::once((&[][..], &source)));
    let canonical = |name: &str| {
        let f = function(&source, name);
        view(&context, &f.sig, &f.block)
    };
    assert_ne!(canonical("gated"), canonical("folded"));
    assert_ne!(canonical("plain"), canonical("bare"));
}
