//! Original-source dependency scheduling contracts.

use dejadoc::{Dejadoc, Kind, SourceFile, TargetScan};
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

fn file(path: &str, segments: &[&str], code: &str) -> SourceFile {
    SourceFile {
        path: path.to_owned(),
        segments: segments
            .iter()
            .map(|segment| (*segment).to_owned())
            .collect(),
        parsed: syn::parse_str(code).unwrap(),
        text: code.to_owned(),
        rustdoc: true,
    }
}

#[test]
fn original_module_files_supply_primitive_alias_facts() {
    let target = TargetScan {
        name: "sample".to_owned(),
        library: true,
        files: vec![
            file("src/lib.rs", &[], "type Word = u32; mod child;"),
            file(
                "src/child.rs",
                &["child"],
                "use super::Word;
                fn original(input: Word) -> (Word, Word) {
                    let low = input & 15u32; let high = input >> 4; (low, high)
                }
                fn scheduled(argument: Word) -> (Word, Word) {
                    let high = argument >> 4; let low = argument & 15u32; (low, high)
                }",
            ),
        ],
    };
    let report =
        Dejadoc::default()
            .functions()
            .fn_min_tokens(0)
            .run_targets("", &[target], &|_, _| None);
    let groups: Vec<_> = report
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Function)
        .map(|group| {
            let mut members: Vec<_> = group.sites.iter().map(|site| site.item.as_str()).collect();
            members.sort_unstable();
            members
        })
        .collect();
    assert_eq!(
        groups,
        vec![vec!["sample::child::original", "sample::child::scheduled"]]
    );
}

#[test]
fn contextual_primitive_operations_cannot_merge_with_shadowed_overloads() {
    let body =
        "fn f(input: u32) -> (u32, u32) { let a = input & 15u32; let b = input >> 4; (a, b) }";
    let native: syn::File = syn::parse_str(body).unwrap();
    let source = format!(
        "
        #[derive(Clone, Copy)] struct u32(::core::primitive::u32);
        impl ::core::ops::BitAnd<::core::primitive::u32> for u32 {{
            type Output = Self;
            fn bitand(self, rhs: ::core::primitive::u32) -> Self {{ Self(self.0 + rhs) }}
        }}
        impl ::core::ops::Shr<::core::primitive::u32> for u32 {{
            type Output = Self;
            fn shr(self, rhs: ::core::primitive::u32) -> Self {{ Self(self.0 + rhs) }}
        }}
        {body}"
    );
    let native_context = SourceContext::new(core::iter::once((&[][..], &native)));
    let native = function(&native, "f");
    let native = native_context
        .function(&native.sig, &native.block)
        .unwrap()
        .canonicalize();
    for source in [
        source.clone(),
        source.replace("struct u32(", "struct r#u32("),
    ] {
        let overloaded: syn::File = syn::parse_str(&source).unwrap();
        let context = SourceContext::new(core::iter::once((&[][..], &overloaded)));
        let overloaded = function(&overloaded, "f");
        assert_ne!(
            native,
            context
                .function(&overloaded.sig, &overloaded.block)
                .unwrap()
                .canonicalize(),
        );
    }
}

#[test]
fn enclosing_attribute_observers_freeze_contextual_declarations() {
    let source: syn::File = syn::parse_str(
        "
        #[observer::inspect]
        mod observed {
            fn a(input: u32) -> (u32, u32) {
                let low = input & 15u32; let high = input >> 4; (low, high)
            }
            fn b(input: u32) -> (u32, u32) {
                let high = input >> 4; let low = input & 15u32; (low, high)
            }
        }
    ",
    )
    .unwrap();
    let context = SourceContext::new(core::iter::once((&[][..], &source)));
    let syn::Item::Mod(module) = &source.items[0] else {
        unreachable!()
    };
    let items = &module.content.as_ref().unwrap().1;
    let forms: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Fn(function) => Some(
                context
                    .function(&function.sig, &function.block)
                    .unwrap()
                    .canonicalize(),
            ),
            _ => None,
        })
        .collect();
    assert_ne!(forms[0], forms[1]);
}

#[test]
fn inline_modules_resolve_their_own_imported_aliases() {
    let target = TargetScan {
        name: "sample".to_owned(),
        library: true,
        files: vec![file(
            "src/lib.rs",
            &[],
            "
            type Word = u32;
            mod child {
                use super::Word;
                fn original(input: Word) -> (Word, Word) {
                    let low = input & 15u32; let high = input >> 4; (low, high)
                }
                fn scheduled(input: Word) -> (Word, Word) {
                    let high = input >> 4; let low = input & 15u32; (low, high)
                }
            }
        ",
        )],
    };
    let report =
        Dejadoc::default()
            .functions()
            .fn_min_tokens(0)
            .run_targets("", &[target], &|_, _| None);
    let groups: Vec<_> = report
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Function)
        .map(|group| {
            let mut members: Vec<_> = group.sites.iter().map(|site| site.item.as_str()).collect();
            members.sort_unstable();
            members
        })
        .collect();
    assert_eq!(
        groups,
        vec![vec!["sample::child::original", "sample::child::scheduled"]]
    );
}

#[test]
fn inline_modules_preserve_their_own_primitive_shadowing() {
    let source: syn::File = syn::parse_str(
        "
        fn native(input: u32) -> (u32, u32) {
            let low = input & 15u32; let high = input >> 4; (low, high)
        }
        mod plugin {
            #[derive(Clone, Copy)] struct r#u32(::core::primitive::u32);
            impl ::core::ops::BitAnd<::core::primitive::u32> for u32 {
                type Output = Self;
                fn bitand(self, rhs: ::core::primitive::u32) -> Self { Self(self.0 + rhs) }
            }
            impl ::core::ops::Shr<::core::primitive::u32> for u32 {
                type Output = Self;
                fn shr(self, rhs: ::core::primitive::u32) -> Self { Self(self.0 + rhs) }
            }
            fn overloaded(input: u32) -> (u32, u32) {
                let low = input & 15u32; let high = input >> 4; (low, high)
            }
        }
    ",
    )
    .unwrap();
    let context = SourceContext::new(core::iter::once((&[][..], &source)));
    let native = function(&source, "native");
    let syn::Item::Mod(module) = &source.items[1] else {
        unreachable!()
    };
    let overloaded = module
        .content
        .as_ref()
        .unwrap()
        .1
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) => Some(function),
            _ => None,
        })
        .unwrap();
    assert_ne!(
        context
            .function(&native.sig, &native.block)
            .unwrap()
            .canonicalize(),
        context
            .function(&overloaded.sig, &overloaded.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn enclosing_generic_parameters_cannot_seed_native_operations() {
    let source: syn::File = syn::parse_str(
        "
        struct Carrier<T>(T);
        impl<u32> Carrier<u32>
        where u32: Copy
            + ::core::ops::BitAnd<::core::primitive::u32, Output = u32>
            + ::core::ops::Shr<::core::primitive::u32, Output = u32>
        {
            fn original(input: u32) -> (u32, u32) {
                let low = input & 15u32; let high = input >> 4; (low, high)
            }
            fn scheduled(input: u32) -> (u32, u32) {
                let high = input >> 4; let low = input & 15u32; (low, high)
            }
        }
        ",
    )
    .unwrap();
    let context = SourceContext::new(core::iter::once((&[][..], &source)));
    let syn::Item::Impl(owner) = &source.items[1] else {
        unreachable!()
    };
    let forms: Vec<_> = owner
        .items
        .iter()
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
    assert_ne!(forms[0], forms[1]);
}

#[test]
fn imported_primitive_paths_preserve_relative_and_absolute_resolution() {
    let source = r#"
        mod core {
            pub mod primitive {
                #[derive(Clone, Copy)]
                pub struct u32(pub ::core::primitive::u32);
                impl ::core::ops::BitAnd<::core::primitive::u32> for u32 {
                    type Output = Self;
                    fn bitand(self, rhs: ::core::primitive::u32) -> Self {
                        println!("and"); Self(self.0 & rhs)
                    }
                }
                impl ::core::ops::Shr<::core::primitive::u32> for u32 {
                    type Output = Self;
                    fn shr(self, rhs: ::core::primitive::u32) -> Self {
                        println!("shr"); Self(self.0 >> rhs)
                    }
                }
            }
        }
        use core::primitive::u32 as Word;
        fn original(input: Word) -> (Word, Word) {
            let low = input & 15u32; let high = input >> 4; (low, high)
        }
        fn scheduled(input: Word) -> (Word, Word) {
            let high = input >> 4; let low = input & 15u32; (low, high)
        }
    "#;
    for namespace in ["core", "std"] {
        let relative = source
            .replace("mod core {", &format!("mod {namespace} {{"))
            .replace("use core::", &format!("use {namespace}::"));
        for absolute in [false, true] {
            let code = if absolute {
                relative.replace(
                    &format!("use {namespace}::"),
                    &format!("use ::{namespace}::"),
                )
            } else {
                relative.clone()
            };
            let parsed: syn::File = syn::parse_str(&code).unwrap();
            let context = SourceContext::new(core::iter::once((&[][..], &parsed)));
            let original = function(&parsed, "original");
            let scheduled = function(&parsed, "scheduled");
            let original = context
                .function(&original.sig, &original.block)
                .unwrap()
                .canonicalize();
            let scheduled = context
                .function(&scheduled.sig, &scheduled.block)
                .unwrap()
                .canonicalize();
            if absolute {
                assert_eq!(original, scheduled);
            } else {
                assert_ne!(original, scheduled);
            }
        }
    }
}

#[test]
fn nested_relative_imports_ignore_unrelated_root_aliases() {
    let mut forms = Vec::new();
    for root_width in ["u16", "u64"] {
        let source: syn::File = syn::parse_str(&format!(
            "
            mod helpers {{ pub type Word = {root_width}; }}
            mod child {{
                mod helpers {{ pub type Word = u32; }}
                use helpers::Word;
                fn value(input: Word) -> Word {{ input }}
            }}
            ",
        ))
        .unwrap();
        let context = SourceContext::new(core::iter::once((&[][..], &source)));
        let syn::Item::Mod(child) = &source.items[1] else {
            unreachable!()
        };
        let value = child
            .content
            .as_ref()
            .unwrap()
            .1
            .iter()
            .find_map(|item| match item {
                syn::Item::Fn(function) => Some(function),
                _ => None,
            })
            .unwrap();
        forms.push(
            context
                .function(&value.sig, &value.block)
                .unwrap()
                .canonicalize(),
        );
    }
    assert_eq!(forms[0], forms[1]);
}

#[test]
fn self_type_expansion_retains_nested_constant_block_facts() {
    let source = "
        trait Factory { fn make() -> Self; }
        fn outer() {
            impl Factory for [u32; { 3u32 } as usize] {
                fn make() -> Self { [0; 3] }
            }
        }
    ";
    let explicit = source.replace(
        "fn make() -> Self { [0; 3] }",
        "fn make() -> [u32; { 3u32 } as usize] { [0; 3] }",
    );
    assert_eq!(
        syn_canon::canonicalize(syn::parse_str(source).unwrap()),
        syn_canon::canonicalize(syn::parse_str(&explicit).unwrap()),
    );
}

fn assert_incomplete_type_scope_retains_order(declarations: &str, ty: &str) {
    let definitions = r#"
        mod custom {
            #[derive(Clone, Copy)]
            pub struct u32(pub ::core::primitive::u32);
            impl ::core::ops::BitAnd<::core::primitive::u32> for u32 {
                type Output = Self;
                fn bitand(self, rhs: ::core::primitive::u32) -> Self {
                    println!("and"); Self(self.0 & rhs)
                }
            }
            impl ::core::ops::Shr<::core::primitive::u32> for u32 {
                type Output = Self;
                fn shr(self, rhs: ::core::primitive::u32) -> Self {
                    println!("shr"); Self(self.0 >> rhs)
                }
            }
        }
    "#;
    let source: syn::File = syn::parse_str(&format!(
        "{definitions} {declarations}
        fn original(input: {ty}) -> ({ty}, {ty}) {{
            let low = input & 15u32; let high = input >> 4; (low, high)
        }}
        fn scheduled(input: {ty}) -> ({ty}, {ty}) {{
            let high = input >> 4; let low = input & 15u32; (low, high)
        }}",
    ))
    .unwrap();
    let context = SourceContext::new(core::iter::once((&[][..], &source)));
    let original = function(&source, "original");
    let scheduled = function(&source, "scheduled");
    assert_ne!(
        context
            .function(&original.sig, &original.block)
            .unwrap()
            .canonicalize(),
        context
            .function(&scheduled.sig, &scheduled.block)
            .unwrap()
            .canonicalize(),
    );
    let mut original_file = source.clone();
    original_file
        .items
        .retain(|item| !matches!(item, syn::Item::Fn(f) if f.sig.ident == "scheduled"));
    let mut scheduled_file = source;
    scheduled_file
        .items
        .retain(|item| !matches!(item, syn::Item::Fn(f) if f.sig.ident == "original"));
    assert_ne!(
        syn_canon::canonicalize(original_file),
        syn_canon::canonicalize(scheduled_file),
    );
}

#[test]
fn imported_aliases_retain_defining_module_glob_uncertainty() {
    assert_incomplete_type_scope_retains_order(
        "mod aliases { use crate::custom::*; pub type Word = u32; } use aliases::Word;",
        "Word",
    );
}

#[test]
fn generated_module_bindings_cannot_seed_primitive_parameters() {
    assert_incomplete_type_scope_retains_order(
        "macro_rules! define { () => { pub use crate::custom::u32; } } define!();",
        "u32",
    );
}

#[test]
fn conditional_type_aliases_cannot_seed_primitive_parameters() {
    assert_incomplete_type_scope_retains_order(
        "#[cfg(not(any()))] type Word = custom::u32; #[cfg(any())] type Word = u32;",
        "Word",
    );
}

#[test]
fn generated_module_bindings_cannot_seed_qualified_primitive_parameters() {
    assert_incomplete_type_scope_retains_order(
        "macro_rules! define { () => { mod core { pub mod primitive { pub use crate::custom::u32; } } } } define!();",
        "core::primitive::u32",
    );
}

/// A borrowed function with an inherited scalar alias and constant.
fn inherited_alias_tail_form(body: &str, width: &str) -> syn_canon::CanonicalForm {
    let code = format!(
        "mod helper {{ pub type W = {width}; pub const WORD: W = 0; }} use helper::{{WORD, W}}; {body}"
    );
    let source: syn::File = syn::parse_str(&code).unwrap();
    let context = SourceContext::new(core::iter::once((&[][..], &source)));
    let f = function(&source, "f");
    context.function(&f.sig, &f.block).unwrap().canonicalize()
}

#[test]
fn inherited_alias_tail_widths_retain_their_resolved_types() {
    let a = inherited_alias_tail_form("fn f() -> W { WORD }", "u32");
    let b = inherited_alias_tail_form("fn f() -> W { WORD }", "u64");
    assert_ne!(a, b, "the tail keeps its resolved alias width");
}

#[test]
fn inherited_alias_tuple_tails_retain_their_resolved_element_types() {
    let a = inherited_alias_tail_form("fn f() -> (W, W) { (WORD, WORD) }", "u32");
    let b = inherited_alias_tail_form("fn f() -> (W, W) { (WORD, WORD) }", "u64");
    assert_ne!(a, b, "the tuple tail keeps its resolved alias widths");
}

fn function_form(source: &str) -> syn_canon::CanonicalForm {
    let parsed: syn::File = syn::parse_str(source).unwrap();
    let context = SourceContext::new(core::iter::once((&[][..], &parsed)));
    let value = function(&parsed, "value");
    context
        .function(&value.sig, &value.block)
        .unwrap()
        .canonicalize()
}

#[test]
fn division_and_remainder_labels_keep_their_operator_kinds() {
    let quotient = function_form(
        "fn value(left: u32, right: u32) -> (u32, u32) { let part = left / right; (part, part) }",
    );
    assert_eq!(
        quotient,
        function_form(
            "fn value(first: u32, second: u32) -> (u32, u32) { let share = first / second; (share, share) }"
        ),
        "the quotient keeps its alpha identity"
    );
    assert_ne!(
        quotient,
        function_form(
            "fn value(left: u32, right: u32) -> (u32, u32) { let part = left % right; (part, part) }"
        ),
        "the remainder keeps a distinct operator kind"
    );
}

#[test]
fn qualified_calls_keep_their_written_const_generic_arguments() {
    let call = |width: &str, input: &str| {
        format!(
            "mod helpers {{
                pub fn take<const N: usize>(value: u32) -> u32 {{ value + N as u32 }}
            }}
            fn value({input}: u32) -> u32 {{ helpers::take::<{width}>({input}) }}"
        )
    };
    let taken = function_form(&call("4", "input"));
    assert_eq!(
        taken,
        function_form(&call("4", "argument")),
        "the argument keeps its alpha identity"
    );
    assert_ne!(
        taken,
        function_form(&call("8", "input")),
        "the written argument keeps its width"
    );
}

#[test]
fn nested_attributed_mutable_locals_keep_their_unproven_reads() {
    let code = |name: &str, width: &str| {
        format!(
            "fn value(input: u32) -> u32 {{
                let tail = {{ #[cfg(all())] let mut {name} = input & 255u32; {name} ^= 1; {name} >> {width} }};
                tail
            }}"
        )
    };
    let nested = function_form(&code("cell", "4"));
    assert_eq!(
        nested,
        function_form(&code("slot", "4")),
        "the nested local keeps its alpha identity"
    );
    assert_ne!(
        nested,
        function_form(&code("cell", "8")),
        "the nested shift keeps its width"
    );
}

#[test]
fn local_type_aliases_keep_distinct_cast_target_identities() {
    let code = |body: &str| {
        let source: syn::File = syn::parse_str(body).unwrap();
        syn_canon::canonicalize(source)
    };
    let scoped = "
        mod wide { pub type Wide = u32; }
        fn value(input: u32) -> (u32, u32) {
            type Slim = u32;
            let a = input & 15u32;
            let b = input >> 4;
            (a as Slim, b as wide::Wide)
        }
    ";
    let renamed = scoped
        .replace(
            "mod wide { pub type Wide = u32; }",
            "mod broad { pub type Broad = u32; }",
        )
        .replace("Slim", "Narrow")
        .replace("wide::Wide", "broad::Broad");
    let reordered = scoped.replace(
        "let a = input & 15u32;\n            let b = input >> 4;",
        "let b = input >> 4;\n            let a = input & 15u32;",
    );
    let primitive = scoped.replace("a as Slim", "a as u32");
    let scoped = code(scoped);
    assert_eq!(scoped, code(&renamed), "the alias keeps its local identity");
    assert_eq!(
        scoped,
        code(&reordered),
        "the producers keep their scheduled order"
    );
    assert_ne!(
        scoped,
        code(&primitive),
        "the alias keeps a distinct cast target"
    );
}

#[test]
fn whitespace_terminated_format_captures_keep_their_producer_ports() {
    let source = "fn value(input: u32) -> String { let low = input & 15u32; let high = input >> 4; format!(\"{low } {high}\") }";
    let reordered = source.replace(
        "let low = input & 15u32; let high = input >> 4;",
        "let high = input >> 4; let low = input & 15u32;",
    );
    let other = source.replace("{low }", "{high }");
    let original = function_form(source);
    assert_eq!(original, function_form(&reordered));
    assert_ne!(original, function_form(&other));
}
