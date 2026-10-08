//! Dependency scheduling through public canonicalization.

#[path = "support/dependency.rs"]
mod support;

fn form(source: &str) -> String {
    syn_canon::canonicalize(syn::parse_str(source).unwrap()).to_string()
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
