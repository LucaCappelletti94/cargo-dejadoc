//! Dependency scheduling through public canonicalization.

#[path = "support/dependency.rs"]
mod support;

fn form(source: &str) -> String {
    syn_canon::canonicalize(syn::parse_str(source).unwrap()).to_string()
}

fn renamed(source: &str, names: &[(&str, &str)]) -> String {
    fn tokens(input: proc_macro2::TokenStream, names: &[(&str, &str)]) -> proc_macro2::TokenStream {
        input
            .into_iter()
            .map(|token| match token {
                proc_macro2::TokenTree::Ident(mut ident) => {
                    if let Some((_, replacement)) = names.iter().find(|(name, _)| ident == *name) {
                        ident = proc_macro2::Ident::new(replacement, ident.span());
                    }
                    proc_macro2::TokenTree::Ident(ident)
                }
                proc_macro2::TokenTree::Group(group) => {
                    let mut renamed =
                        proc_macro2::Group::new(group.delimiter(), tokens(group.stream(), names));
                    renamed.set_span(group.span());
                    proc_macro2::TokenTree::Group(renamed)
                }
                token => token,
            })
            .collect()
    }
    tokens(source.parse().unwrap(), names).to_string()
}

fn scheduled(id: &str) {
    let case = support::cases()
        .iter()
        .find(|case| case.id == id && case.family == "schedule" && case.expected == "same")
        .unwrap();
    assert_eq!(form(&case.a), form(&case.b), "scheduled forms of {id}");
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(&case.a).unwrap()).to_string(),
        syn_canon::canonicalize_failing(syn::parse_str(&case.b).unwrap()).to_string(),
        "failing forms of {id}"
    );
}

#[test]
fn graph_scalar_declarations() {
    scheduled("D01_literals");
}
#[test]
fn graph_common_input() {
    scheduled("D02_bitwise");
}
#[test]
fn graph_interleaved_chains() {
    scheduled("D03_chains");
}
#[test]
fn graph_renamed_schedule() {
    scheduled("D04_renamed");
}
#[test]
fn graph_repeated_producers() {
    scheduled("D05_duplicate_labels");
}
#[test]
fn graph_shadowing_origins() {
    scheduled("D06_shadowing");
}
#[test]
fn graph_literal_provenance() {
    scheduled("D08_literal_provenance");
}
#[test]
fn graph_observation_barriers() {
    scheduled("D09_effect_barrier");
}

#[test]
fn graph_preserves_literal_type_and_parameter_ports() {
    let source = "fn f(first: u32, second: u32) -> (u32, u32) { let a = first & 15u32; let b = second >> 4; (a, b) }";
    for changed in [
        source.replace("15u32", "14u32"),
        source.replace("u32", "u64"),
        source
            .replace("first &", "second &")
            .replace("second >>", "first >>"),
    ] {
        assert_ne!(form(source), form(&changed));
    }
}

#[test]
fn graph_preserves_unknown_observer_schedules() {
    let a =
        "macro_rules! observer { () => {} } fn f() { let a = 1u32; let b = 2u32; observer!(); }";
    let b =
        "macro_rules! observer { () => {} } fn f() { let b = 2u32; let a = 1u32; observer!(); }";
    assert_ne!(form(a), form(b));
}

#[test]
fn graph_preserves_unsafe_shift_schedules() {
    let a = "fn f(input: u32, shift: u32) -> (u32, u32) { let a = input << shift; let b = input & 15u32; (a, b) }";
    let b = "fn f(input: u32, shift: u32) -> (u32, u32) { let b = input & 15u32; let a = input << shift; (a, b) }";
    assert_ne!(form(a), form(b));
}

#[test]
fn graph_preserves_storage_observed_after_declarations() {
    let a = "fn f() -> bool { let a = 1u32; let b = 2u32; std::ptr::eq(&a, &b) }";
    let b = "fn f() -> bool { let b = 2u32; let a = 1u32; std::ptr::eq(&a, &b) }";
    assert_ne!(form(a), form(b));
}

#[test]
fn graph_preserves_unreachable_suffix_schedules() {
    let a = "fn f(input: u32) -> u32 { return input; let a = 1u32; let b = 2u32; a ^ b }";
    let b = "fn f(input: u32) -> u32 { return input; let b = 2u32; let a = 1u32; a ^ b }";
    assert_ne!(form(a), form(b));
}

#[test]
fn graph_freezes_ancestor_bindings_for_empty_nested_observers() {
    let a = "macro_rules! observer { () => {} } fn f() -> (u32, u32) { let a = 1u32; let b = 2u32; { observer!(); } (a, b) }";
    let b = "macro_rules! observer { () => {} } fn f() -> (u32, u32) { let b = 2u32; let a = 1u32; { observer!(); } (a, b) }";
    assert_ne!(form(a), form(b));
}

#[test]
fn graph_preserves_reads_across_mutable_value_versions() {
    let a = "fn f() -> (u32, u32) { let mut value = 1u32; let first = value & 15u32; value = 2u32; let second = value >> 4; (first, second) }";
    let b = "fn f() -> (u32, u32) { let mut value = 1u32; value = 2u32; let first = value & 15u32; let second = value >> 4; (first, second) }";
    assert_ne!(form(a), form(b));
}

#[test]
fn graph_preserves_shadowing_reference_origins() {
    let a = "fn f(input: u32) -> (u32, u32) { let value = input & 15u32; let first = value; let value = value ^ 3u32; (first, value) }";
    let b = "fn f(input: u32) -> (u32, u32) { let value = input & 15u32; let value = value ^ 3u32; let first = value; (first, value) }";
    assert_ne!(form(a), form(b));
}

#[test]
fn graph_scheduling_is_independent_of_constructed_ast_spans() {
    fn shared(stream: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
        stream
            .into_iter()
            .map(|mut token| {
                if let proc_macro2::TokenTree::Group(group) = token {
                    token = proc_macro2::TokenTree::Group(proc_macro2::Group::new(
                        group.delimiter(),
                        shared(group.stream()),
                    ));
                }
                token.set_span(proc_macro2::Span::call_site());
                token
            })
            .collect()
    }
    let case = support::cases()
        .iter()
        .find(|case| case.id == "D05_duplicate_labels")
        .unwrap();
    let a = syn_canon::canonicalize(syn::parse2(shared(case.a.parse().unwrap())).unwrap());
    let b = syn_canon::canonicalize(syn::parse2(shared(case.b.parse().unwrap())).unwrap());
    assert_eq!(a.to_string(), b.to_string());
}

#[test]
fn graph_preserves_parameter_types_under_unresolved_module_globs() {
    let a = "mod plugin; use plugin::*; fn f(input: u32) -> (u32, u32) { let a = input & 15u32; let b = input >> 4; (a, b) }";
    let b = "mod plugin; use plugin::*; fn f(input: u32) -> (u32, u32) { let b = input >> 4; let a = input & 15u32; (a, b) }";
    assert_ne!(form(a), form(b));
}

#[test]
fn graph_preserves_parameter_types_under_generic_shadowing() {
    let a = "fn f<u32>(input: u32) -> (u32, u32) where u32: Copy + ::core::ops::BitAnd<::core::primitive::u32, Output = u32> + ::core::ops::Shr<::core::primitive::u32, Output = u32> { let a = input & 15u32; let b = input >> 4; (a, b) }";
    let b = "fn f<u32>(input: u32) -> (u32, u32) where u32: Copy + ::core::ops::BitAnd<::core::primitive::u32, Output = u32> + ::core::ops::Shr<::core::primitive::u32, Output = u32> { let b = input >> 4; let a = input & 15u32; (a, b) }";
    assert_ne!(form(a), form(b));
}

#[test]
fn graph_preserves_implicit_format_capture_ports() {
    let a = "fn f(input: u32) { let low = input & 15u32; let high = input >> 4; println!(\"{low} {high}\"); }";
    let b = "fn f(input: u32) { let high = input >> 4; let low = input & 15u32; println!(\"{low} {high}\"); }";
    let changed = "fn f(input: u32) { let high = input >> 4; let low = input & 15u32; println!(\"{high} {low}\"); }";
    assert_eq!(form(a), form(b));
    assert_ne!(form(a), form(changed));
}

#[test]
fn graph_preserves_dynamic_format_width_and_precision_ports() {
    let a = "fn f(input: u32, width: usize, precision: usize) { let low = input & 15u32; let high = input >> 4; println!(\"{low:width$} {high:.precision$}\"); }";
    let b = "fn f(input: u32, width: usize, precision: usize) { let high = input >> 4; let low = input & 15u32; println!(\"{low:width$} {high:.precision$}\"); }";
    let changed = "fn f(input: u32, width: usize, precision: usize) { let high = input >> 4; let low = input & 15u32; println!(\"{low:precision$} {high:.width$}\"); }";
    assert_eq!(form(a), form(b));
    assert_ne!(form(a), form(changed));
}

#[test]
fn imported_macro_names_cannot_inherit_standard_observer_proofs() {
    let a = "mod plugin; use plugin::println; fn f() { let a = 1u32; let b = 2u32; println!(); }";
    let b = "mod plugin; use plugin::println; fn f() { let b = 2u32; let a = 1u32; println!(); }";
    assert_ne!(form(a), form(b));
}

#[test]
fn unresolved_macro_globs_freeze_literal_producer_schedules() {
    let a = "mod plugin; use plugin::*; fn f() { let a = 1u32; let b = 2u32; println!(); }";
    let b = "mod plugin; use plugin::*; fn f() { let b = 2u32; let a = 1u32; println!(); }";
    assert_ne!(form(a), form(b));
}

#[test]
fn block_local_macro_imports_preserve_unknown_observer_order() {
    let a = "mod plugin; fn f() { use plugin::println; let a = 1u32; let b = 2u32; println!(); }";
    let b = "mod plugin; fn f() { use plugin::println; let b = 2u32; let a = 1u32; println!(); }";
    assert_ne!(form(a), form(b));
}

#[test]
fn block_local_macro_globs_preserve_unknown_observer_order() {
    let a = "mod plugin; fn f() { use plugin::*; let a = 1u32; let b = 2u32; println!(); }";
    let b = "mod plugin; fn f() { use plugin::*; let b = 2u32; let a = 1u32; println!(); }";
    assert_ne!(form(a), form(b));
}

#[test]
fn reordered_same_name_literals_retain_the_selected_origin() {
    let first = "fn f() -> u32 { let value = 2u32; let value = 1u32; value }";
    let second = "fn f() -> u32 { let value = 1u32; let value = 2u32; value }";
    assert_ne!(form(first), form(second));
}

#[test]
fn qualified_primitive_names_retain_their_lexical_module_origin() {
    let prefix = "mod core { pub mod primitive {
        #[derive(Clone, Copy)] pub struct u32(pub ::core::primitive::u32);
        impl ::core::ops::BitAnd<::core::primitive::u32> for u32 {
            type Output = Self;
            fn bitand(self, rhs: ::core::primitive::u32) -> Self { Self(self.0 & rhs) }
        }
        impl ::core::ops::Shr<::core::primitive::u32> for u32 {
            type Output = Self;
            fn shr(self, rhs: ::core::primitive::u32) -> Self { Self(self.0 >> rhs) }
        }
    } }";
    let first = format!(
        "{prefix} fn f(input: core::primitive::u32)
        -> (core::primitive::u32, core::primitive::u32) {{
        let low = input & 15u32; let high = input >> 4; (low, high) }}"
    );
    let second = format!(
        "{prefix} fn f(input: core::primitive::u32)
        -> (core::primitive::u32, core::primitive::u32) {{
        let high = input >> 4; let low = input & 15u32; (low, high) }}"
    );
    assert_ne!(form(&first), form(&second));
}

#[test]
fn implicit_capture_roles_distinguish_repeated_producers() {
    let first = "fn f(input: u32) { let left = input >> 4; let right = input >> 4; println!(\"{left} {right}\"); }";
    let second = "fn f(input: u32) { let right = input >> 4; let left = input >> 4; println!(\"{left} {right}\"); }";
    let repeated = "fn f(input: u32) { let left = input >> 4; let right = input >> 4; println!(\"{left} {left}\"); }";
    assert_eq!(form(first), form(second));
    assert_ne!(form(first), form(repeated));
}

#[test]
fn possible_divergence_preserves_reachable_scalar_regions() {
    let first = "fn f(input: u32) -> (u32, u32) { assert!(input > 0); let low = input & 15u32; let high = input >> 4; (low, high) }";
    let second = "fn f(input: u32) -> (u32, u32) { assert!(input > 0); let high = input >> 4; let low = input & 15u32; (low, high) }";
    assert_eq!(form(first), form(second));
}

#[test]
fn observers_inside_formatted_arguments_freeze_the_visible_scope() {
    let first =
        "fn f() { let first = 1u32; let second = 2u32; println!(\"{}\", stringify!(first)); }";
    let second =
        "fn f() { let second = 2u32; let first = 1u32; println!(\"{}\", stringify!(first)); }";
    assert_ne!(form(first), form(second));
}

#[test]
fn disconnected_islands_preserve_earlier_binding_origins() {
    let original = r#"
        fn f() -> u32 {
            let first = 1u32;
            let second = 1u32;
            println!("{} {}", first, second);
            let left = first ^ 2u32;
            let right = second ^ 2u32;
            0u32
        }
    "#;
    let scheduled = original.replace(
        "let left = first ^ 2u32;\n            let right = second ^ 2u32;",
        "let right = second ^ 2u32;\n            let left = first ^ 2u32;",
    );
    assert_eq!(form(original), form(&scheduled));
    let shared = original.replace("let right = second ^", "let right = first ^");
    assert_ne!(form(original), form(&shared));
}

#[test]
fn graph_preserves_constant_shift_width_boundary() {
    let source = |count, reversed| {
        let shift = format!("let high = input >> {count}u32;");
        let mask = "let low = input & 15u32;";
        let declarations = if reversed {
            format!("{mask}{shift}")
        } else {
            format!("{shift}{mask}")
        };
        format!(
            "#[allow(arithmetic_overflow)] fn f(input:u32)->(u32,u32){{{declarations}(low,high)}}"
        )
    };
    assert_eq!(form(&source(31, false)), form(&source(31, true)));
    assert_ne!(form(&source(32, false)), form(&source(32, true)));
}

fn assert_storage_observation_retains_order(observation: &str) {
    let source = |reversed| {
        let declarations = if reversed {
            "let b=2u32; let a=1u32;"
        } else {
            "let a=1u32; let b=2u32;"
        };
        format!("fn f()->(u32,u32){{{declarations}{observation}(a,b)}}")
    };
    assert_ne!(form(&source(false)), form(&source(true)));
}

#[test]
fn raw_addresses_freeze_producers_before_a_scalar_tail() {
    assert_storage_observation_retains_order(
        "let pointer = &raw const a; std::hint::black_box(pointer);",
    );
}

#[test]
fn closure_captures_freeze_producers_before_a_scalar_tail() {
    assert_storage_observation_retains_order("let capture = || a; std::hint::black_box(capture);");
}

#[test]
fn method_receivers_freeze_producers_before_a_scalar_tail() {
    assert_storage_observation_retains_order("a.to_string();");
}

#[test]
fn qualified_native_parameters_preserve_legal_schedules() {
    for (prefix, ty) in [
        ("", "core::primitive::u32"),
        (
            "mod core { pub mod primitive { pub struct u32; } }",
            "::core::primitive::u32",
        ),
    ] {
        let source = |reversed| {
            let declarations = if reversed {
                "let high=input>>4u32;let low=input&15u32;"
            } else {
                "let low=input&15u32;let high=input>>4u32;"
            };
            format!("{prefix} fn f(input: {ty})->({ty},{ty}){{{declarations}(low,high)}}")
        };
        assert_eq!(form(&source(false)), form(&source(true)));
    }
}

#[test]
fn disconnected_symmetric_chains_preserve_legal_schedules() {
    let original = "fn f(input:u32)->u32 {
        let a=input&7u32; let b=input&7u32;
        let c=a^2u32; let d=b^2u32; 0u32
    }";
    let scheduled = "fn f(value:u32)->u32 {
        let right=value&7u32; let right_child=right^2u32;
        let left=value&7u32; let left_child=left^2u32; 0u32
    }";
    assert_eq!(form(original), form(scheduled));
}

#[test]
fn heterogeneous_scalar_schedules_preserve_statement_types() {
    let original = "fn f() -> (u32, bool) { let first = 1u32; let second = true; (first, second) }";
    let scheduled =
        "fn f() -> (u32, bool) { let second = true; let first = 1u32; (first, second) }";
    assert_eq!(form(original), form(scheduled));
}

#[test]
fn nested_closure_inputs_have_distinct_lexical_origins() {
    let original = "fn f() { let action = |outer: u32| { let nested = |inner: u32| { let first = outer & 15u32; let second = inner & 15u32; 0u32 }; nested(0u32) }; action(0u32); }";
    let scheduled = original.replace(
        "let first = outer & 15u32; let second = inner & 15u32;",
        "let second = inner & 15u32; let first = outer & 15u32;",
    );
    assert_eq!(form(original), form(&scheduled));
}

#[test]
fn equal_literal_producers_match_ordered_opaque_consumer_ports() {
    let source = |reversed| {
        let declarations = if reversed {
            "let d=1u32; let c=1u32; let b=1u32; let a=1u32;"
        } else {
            "let a=1u32; let b=1u32; let c=1u32; let d=1u32;"
        };
        format!("fn f()->u32 {{{declarations} std::hint::black_box((a, b, c, d)); 0u32}}")
    };
    let original = source(false);
    assert_eq!(
        form(&original),
        form(&source(true)),
        "a regular port graph schedules canonically"
    );
    let skewed = original.replace("black_box((a, b, c, d))", "black_box((a, a, c, d))");
    assert_ne!(
        form(&original),
        form(&skewed),
        "the consumer port graph is tracked"
    );
}

#[test]
fn assigned_mutable_producers_keep_their_reassignment_region_in_source_order() {
    for assignment in ["first = 3u32;", "first += 3u32;"] {
        let source = |reversed| {
            let declarations = if reversed {
                "let second=2u32; let mut first=1u32;"
            } else {
                "let mut first=1u32; let second=2u32;"
            };
            format!("fn f()->(u32,u32){{{declarations}{assignment}(first,second)}}")
        };
        assert_ne!(
            form(&source(false)),
            form(&source(true)),
            "a reassignment keeps the region in source order"
        );
    }
}

#[test]
fn statement_level_non_assignment_binaries_keep_their_region_live() {
    let source = |reversed| {
        let declarations = if reversed {
            "let second=input>>4; let first=input&15u32;"
        } else {
            "let first=input&15u32; let second=input>>4;"
        };
        format!("fn f(input:u32)->(u32,u32){{{declarations} first^second; (first,second)}}")
    };
    assert_eq!(
        form(&source(false)),
        form(&source(true)),
        "a non-assignment statement keeps the region live"
    );
}

#[test]
fn qualified_static_assignments_keep_the_same_spelled_value_region_live() {
    let source = |reversed| {
        let declarations = if reversed {
            "let high=input>>4; let m=input&15u32;"
        } else {
            "let m=input&15u32; let high=input>>4;"
        };
        format!(
            "mod m {{ pub static mut GLOBAL: u32 = 0; }} fn f(input:u32)->(u32,u32){{\n// SAFETY: This single-threaded fixture creates no references to `GLOBAL`.\nunsafe {{ {declarations} m::GLOBAL = 1; (m,high) }} }}"
        )
    };
    assert_eq!(
        form(&source(false)),
        form(&source(true)),
        "a qualified assignment does not target the value"
    );
}

#[test]
fn local_producer_format_width_and_precision_keep_their_capture_schedule() {
    let source = |reversed| {
        let declarations = if reversed {
            "let p=1u32; let w=1u32; let low_x=1u32;"
        } else {
            "let low_x=1u32; let w=1u32; let p=1u32;"
        };
        format!(
            "fn f()->u32 {{{declarations} println!(\"{{low_x:w$.p$}}\", w=w as usize, p=p as usize); 0u32}}"
        )
    };
    let original = source(false);
    assert_eq!(
        form(&original),
        form(&source(true)),
        "producer width and precision keep their captures"
    );
    let swapped = original.replace("{low_x:w$.p$}", "{low_x:p$.w$}");
    assert_ne!(
        form(&original),
        form(&swapped),
        "the width and precision names are tracked"
    );
}

#[test]
fn edition_2024_let_chain_conditions_keep_the_source_order_invariance() {
    let source = |reversed| {
        let declarations = if reversed {
            "let b=input>>4; let a=input&15u32;"
        } else {
            "let a=input&15u32; let b=input>>4;"
        };
        format!(
            "fn f(input:u32)->(u32,u32){{{declarations} while let limit=a && let guard=b && limit>guard {{ break; }} (a,b)}}"
        )
    };
    let original = source(false);
    assert_eq!(
        form(&original),
        form(&source(true)),
        "a let chain condition schedules canonically"
    );
    let plain = original.replace("let limit=a && let guard=b && limit>guard", "a>b");
    assert_ne!(
        form(&original),
        form(&plain),
        "the let chain condition is labeled"
    );
}

#[test]
fn qualified_diverging_macros_keep_their_boundary_proofs() {
    for path in [
        "core::assert",
        "std::assert",
        "::core::assert",
        "::std::assert",
    ] {
        let source =
            |body: &str| format!("fn f(input: u32) -> (u32, u32) {{ {path}!(input > 0); {body} }}");
        let original = form(&source(
            "let low = input & 15u32; let high = input >> 4; (low, high)",
        ));
        assert_eq!(
            original,
            form(&source(
                "let high = input >> 4; let low = input & 15u32; (low, high)",
            ))
        );
        assert_eq!(
            original,
            form(&source(
                "let first = input & 15u32; let second = input >> 4; (first, second)",
            ))
        );
    }
}

#[test]
fn foreign_macro_heads_keep_their_unproven_regions() {
    let source = |body: &str| {
        format!("fn f(input: u32) -> (u32, u32) {{ plugin::assert!(input > 0); {body} }}")
    };
    let original = form(&source(
        "let low = input & 15u32; let high = input >> 4; (low, high)",
    ));
    assert_ne!(
        original,
        form(&source(
            "let high = input >> 4; let low = input & 15u32; (low, high)",
        ))
    );
    assert_eq!(
        original,
        form(&source(
            "let first = input & 15u32; let second = input >> 4; (first, second)",
        ))
    );
}

#[test]
fn resolved_module_heads_keep_their_macro_regions_unproven() {
    let source = |body: &str| {
        format!(
            "mod core {{ pub use std::assert; }}
             fn f(input: u32) -> (u32, u32) {{ core::assert!(input > 0); {body} }}"
        )
    };
    let original = form(&source(
        "let low = input & 15u32; let high = input >> 4; (low, high)",
    ));
    assert_ne!(
        original,
        form(&source(
            "let high = input >> 4; let low = input & 15u32; (low, high)",
        ))
    );
    assert_eq!(
        original,
        form(&source(
            "let first = input & 15u32; let second = input >> 4; (first, second)",
        ))
    );
}

#[test]
fn type_named_macros_keep_their_observer_proofs() {
    let source = |body: &str| {
        format!("struct eprintln; fn f(input: u32) -> u32 {{ {body} eprintln!(); 0u32 }}")
    };
    assert_eq!(
        form(&source("let low = input & 15u32; let high = input >> 4;")),
        form(&source("let high = input >> 4; let low = input & 15u32;"))
    );
}

#[test]
fn underscored_captures_keep_their_producer_ports() {
    let source =
        |body: &str| format!("fn f(input: u32) -> u32 {{ {body} println!(\"{{low_x}}\"); 0u32 }}");
    let original = source("let low_x = input >> 4; let high_x = input >> 4;");
    assert_eq!(
        form(&original),
        form(&source("let high_x = input >> 4; let low_x = input >> 4;"))
    );
    assert_eq!(
        form(&original),
        form(
            &original
                .replace("low_x", "first_value")
                .replace("high_x", "second_value")
        )
    );
    assert_ne!(
        form(&original),
        form(&original.replace("{low_x}", "{low_x:x}"))
    );
}

#[test]
fn left_shifts_keep_their_proven_totals() {
    let source = |body: &str| format!("fn f(input: u32) -> (u32, u32) {{ {body} }}");
    let original = form(&source(
        "let left = input << 1u32; let high = input >> 4u32; (left, high)",
    ));
    assert_eq!(
        original,
        form(&source(
            "let high = input >> 4u32; let left = input << 1u32; (left, high)",
        ))
    );
    assert_eq!(
        original,
        form(&source(
            "let first = input << 1u32; let second = input >> 4u32; (first, second)",
        ))
    );
}

#[test]
fn unsuffixed_literal_operands_keep_their_proven_totals() {
    let source = |body: &str| format!("fn f(input: u32) -> (u32, u32) {{ {body} }}");
    for (original, reordered, renamed) in [
        (
            "let low = input & 15; let high = input >> 4; (low, high)",
            "let high = input >> 4; let low = input & 15; (low, high)",
            "let first = input & 15; let second = input >> 4; (first, second)",
        ),
        (
            "let low = 15 & input; let high = 7 ^ input; (low, high)",
            "let high = 7 ^ input; let low = 15 & input; (low, high)",
            "let first = 15 & input; let second = 7 ^ input; (first, second)",
        ),
    ] {
        assert_eq!(form(&source(original)), form(&source(reordered)));
        assert_eq!(form(&source(original)), form(&source(renamed)));
    }
}

#[test]
fn parenthesized_initializers_keep_their_proven_totals() {
    let source = |body: &str| format!("fn f(input: u32) -> (u32, u32) {{ {body} }}");
    assert_eq!(
        form(&source(
            "let low = (input & 15u32); let high = input >> 4u32; (low, high)",
        )),
        form(&source(
            "let high = input >> 4u32; let low = (input & 15u32); (low, high)",
        ))
    );
}

#[test]
fn nested_regions_retain_the_scheduled_identity_of_outer_producers() {
    let source = |outer_reversed: bool, inner_reversed: bool| {
        let outer = if outer_reversed {
            "let high = input >> 4; let low = input & 15u32;"
        } else {
            "let low = input & 15u32; let high = input >> 4;"
        };
        let inner = if inner_reversed {
            "let right = high & 7u32; let left = low & 7u32;"
        } else {
            "let left = low & 7u32; let right = high & 7u32;"
        };
        format!("fn f(input: u32) -> (u32, u32) {{ {outer} {{ {inner} (left, right) }} }}")
    };
    let original = source(false, false);
    for (outer, inner) in [(true, false), (false, true), (true, true)] {
        assert_eq!(form(&original), form(&source(outer, inner)));
    }
    assert_eq!(
        form(&original),
        form(&renamed(
            &original,
            &[
                ("low", "lower"),
                ("high", "upper"),
                ("left", "first"),
                ("right", "second")
            ],
        ))
    );
    assert_ne!(
        form(&original),
        form(&original.replace("(left, right)", "(right, left)"))
    );
}

#[test]
fn nested_storage_observations_preserve_producer_declaration_order() {
    let source = |body: &str, reversed: bool| {
        let declarations = if reversed {
            "let high = 5u32; let low = 3u32;"
        } else {
            "let low = 3u32; let high = 5u32;"
        };
        format!(
            "fn consume<T>(_: T) {{}}
             struct Holder<'a> {{ field: &'a u32 }}
             fn f(flag: bool) {{ {declarations} {body} }}"
        )
    };
    assert_eq!(
        form(&source("consume(low);", false)),
        form(&source("consume(low);", true))
    );
    for body in [
        "let _ = *(&low);",
        "#[allow(unused_parens)] (&low);",
        "let _ = { &low };",
        "if flag { let _ = &low; }",
        "for _ in [()] { let _ = &low; }",
        "while flag { let _ = &low; break; }",
        "loop { let _ = &low; break; }",
        "match () { () => { let _ = &low; } }",
        "let _ = [&low];",
        "let _ = [&low; 2];",
        "let _ = &low..&high;",
        "let _ = [&low][0];",
        "let _ = (&low,).0;",
        "let _ = Holder { field: &low };",
        "let _ = std::assert_eq!(&low, &3u32);",
        "return consume(&low);",
        "let _ = loop { break &low; };",
        "let _ = unsafe { &low };",
        "let _ = async { let _ = &low; };",
    ] {
        let original = source(body, false);
        assert_ne!(form(&original), form(&source(body, true)), "{body}");
        assert_eq!(
            form(&original),
            form(&renamed(&original, &[("low", "first"), ("high", "second")])),
            "{body}"
        );
    }
}

#[test]
fn control_flow_payload_borrows_preserve_producer_declaration_order() {
    let source = |body: &str, reversed: bool| {
        let declarations = if reversed {
            "let high = 5u32; let low = 3u32;"
        } else {
            "let low = 3u32; let high = 5u32;"
        };
        format!(
            "fn consume<T>(_: T) -> Result<(), ()> {{ Ok(()) }}
             fn f() -> Result<(), ()> {{ {declarations} {body} }}"
        )
    };
    for body in [
        "return consume(&low); consume(())",
        "consume(&low)?; Ok(())",
    ] {
        let original = source(body, false);
        assert_ne!(form(&original), form(&source(body, true)), "{body}");
        assert_eq!(
            form(&original),
            form(&renamed(&original, &[("low", "first"), ("high", "second")])),
            "{body}"
        );
    }
    assert_eq!(
        form(&source("consume(low)?; Ok(())", false)),
        form(&source("consume(low)?; Ok(())", true))
    );
}

#[test]
fn awaited_borrows_preserve_producer_declaration_order() {
    let source = |borrowed: bool, reversed: bool| {
        let declarations = if reversed {
            "let high = 5u32; let low = 3u32;"
        } else {
            "let low = 3u32; let high = 5u32;"
        };
        let value = if borrowed { "&low" } else { "low" };
        format!("async fn f() {{ {declarations} let _ = async {{ {value} }}.await; }}")
    };
    let original = source(true, false);
    assert_ne!(form(&original), form(&source(true, true)));
    assert_eq!(
        form(&original),
        form(&renamed(&original, &[("low", "first"), ("high", "second")]))
    );
    assert_eq!(form(&source(false, false)), form(&source(false, true)));
}

#[test]
fn mixed_primitive_overloads_preserve_observable_operator_order() {
    for (implementation, first, second) in [
        (
            "impl std::ops::BitAnd<Scalar> for u32 {
                 type Output = u32;
                 fn bitand(self, rhs: Scalar) -> u32 {
                     std::println!(\"{self}\");
                     self & rhs.0
                 }
             }",
            "1u32 & argument",
            "2u32 & argument",
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
            "argument & 2u32",
        ),
    ] {
        let source = |reversed: bool| {
            let declarations = if reversed {
                format!("let second = {second}; let first = {first};")
            } else {
                format!("let first = {first}; let second = {second};")
            };
            format!(
                "struct Scalar(u32);
                 impl Copy for Scalar {{}}
                 impl Clone for Scalar {{ fn clone(&self) -> Self {{ *self }} }}
                 {implementation}
                 fn f(argument: Scalar) -> (u32, u32) {{ {declarations} (first, second) }}"
            )
        };
        let original = source(false);
        assert_ne!(form(&original), form(&source(true)));
        assert_eq!(
            form(&original),
            form(
                &original
                    .replace("argument", "input")
                    .replace("first", "left")
                    .replace("second", "right")
            )
        );
    }
}
