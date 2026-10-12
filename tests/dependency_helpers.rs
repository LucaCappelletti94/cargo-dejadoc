//! Local helper scheduling through public comparison.

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

fn failing(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize_failing(syn::parse_str(source).unwrap())
}

fn distinct(a: &str, b: &str) {
    assert_ne!(form(a), form(b), "{a}\n{b}");
    assert_ne!(failing(a), failing(b), "{a}\n{b}");
}

/// A pair that keeps its source order, both in the passing and the failing form.
fn ordered(a: &str, b: &str) {
    assert_ne!(form(a), form(b), "{a}\n{b}");
    assert_ne!(failing(a), failing(b), "{a}\n{b}");
}

/// A pair whose passing form schedules away the difference, failing apart.
fn scheduled(a: &str, b: &str) {
    assert_eq!(form(a), form(b), "{a}\n{b}");
    assert_ne!(failing(a), failing(b), "{a}\n{b}");
}

#[test]
fn helper_origins_keep_different_results_distinct() {
    let prefix = "fn low(x:u32)->u32{x&15u32} fn high(x:u32)->u32{x>>4}";
    distinct(
        &format!("{prefix} fn f(x:u32)->u32{{low(x)}}"),
        &format!("{prefix} fn f(x:u32)->u32{{high(x)}}"),
    );
}

#[test]
fn helper_results_with_distinct_widths_stay_distinct() {
    distinct(
        "fn low(x:u32)->u32{x&15u32} fn f(x:u32)->u32{low(x)}",
        "fn low(x:u64)->u64{x&15u64} fn f(x:u64)->u64{low(x)}",
    );
}

#[test]
fn helper_purity_does_not_prove_argument_purity() {
    let prefix = "fn pure(x:u32)->u32{x&15u32} fn effect(x:u32)->u32{println!(\"{x}\");x}";
    distinct(
        &format!(
            "{prefix} fn f()->(u32,u32){{let a=pure(effect(1u32));let b=pure(effect(2u32));(a,b)}}"
        ),
        &format!(
            "{prefix} fn f()->(u32,u32){{let b=pure(effect(2u32));let a=pure(effect(1u32));(a,b)}}"
        ),
    );
}

#[test]
fn recursive_and_diverging_helpers_keep_call_order() {
    for body in ["helper(x)", "loop {}", "panic!(\"stop\")"] {
        let prefix = format!("fn helper(x:u32)->u32{{{body}}}");
        ordered(
            &format!(
                "{prefix} fn f(x:u32)->(u32,u32){{let a=helper(x);let b=helper(x^1u32);(a,b)}}"
            ),
            &format!(
                "{prefix} fn f(x:u32)->(u32,u32){{let b=helper(x^1u32);let a=helper(x);(a,b)}}"
            ),
        );
    }
}

#[test]
fn indirect_recursion_keeps_call_order() {
    ordered(
        "fn even(x:u32)->u32{odd(x)} fn odd(x:u32)->u32{even(x)} fn f(x:u32)->(u32,u32){let a=even(x);let b=odd(x^1u32);(a,b)}",
        "fn even(x:u32)->u32{odd(x)} fn odd(x:u32)->u32{even(x)} fn f(x:u32)->(u32,u32){let b=odd(x^1u32);let a=even(x);(a,b)}",
    );
}

#[test]
fn helpers_that_are_not_total_keep_call_order() {
    for body in [
        "x+1u32",
        "1u32/x",
        "x-y",
        "x<<y",
        "x%2u32",
        "x.checked_add(1u32).unwrap()",
    ] {
        let prefix = format!("fn low(x:u32,y:u32)->u32{{{body}}}");
        ordered(
            &format!("{prefix} fn f(x:u32)->(u32,u32){{let a=low(x,x);let b=low(x^1u32,x);(a,b)}}"),
            &format!("{prefix} fn f(x:u32)->(u32,u32){{let b=low(x^1u32,x);let a=low(x,x);(a,b)}}"),
        );
    }
}

#[test]
fn helpers_with_rejected_grammar_keep_call_order() {
    for body in ["match x{0u32=>1u32,_=>2u32}", "let c=|v:u32|v;c(x)"] {
        let prefix = format!("fn low(x:u32)->u32{{{body}}}");
        ordered(
            &format!("{prefix} fn f(x:u32)->(u32,u32){{let a=low(x);let b=low(x^1u32);(a,b)}}"),
            &format!("{prefix} fn f(x:u32)->(u32,u32){{let b=low(x^1u32);let a=low(x);(a,b)}}"),
        );
    }
    // Type-invalid conditions and branches only have a defined failing form.
    for body in ["if x{1u32}else{2u32}", "if x<2u32{1u32}else{2u64}"] {
        let prefix = format!("fn low(x:u32)->u32{{{body}}}");
        let a = format!("{prefix} fn f(x:u32)->(u32,u32){{let a=low(x);let b=low(x^1u32);(a,b)}}");
        let b = format!("{prefix} fn f(x:u32)->(u32,u32){{let b=low(x^1u32);let a=low(x);(a,b)}}");
        assert_ne!(failing(&a), failing(&b), "{a}\n{b}");
    }
}

#[test]
fn helpers_with_rejected_signatures_keep_call_order() {
    ordered(
        "fn low(x:&u32)->u32{*x} fn f(x:u32)->(u32,u32){let v=x;let w=x^1u32;let a=low(&v);let b=low(&w);(a,b)}",
        "fn low(x:&u32)->u32{*x} fn f(x:u32)->(u32,u32){let v=x;let w=x^1u32;let b=low(&w);let a=low(&v);(a,b)}",
    );
    ordered(
        "fn low<T>(x:T)->T{x} fn f(x:u32)->(u32,u32){let a=low(x);let b=low(x^1u32);(a,b)}",
        "fn low<T>(x:T)->T{x} fn f(x:u32)->(u32,u32){let b=low(x^1u32);let a=low(x);(a,b)}",
    );
    ordered(
        "fn low(x:u32,y:u32)->u32{x^y} fn f(x:u32)->(u32,u32){let a=low(x);let b=low(x^1u32);(a,b)}",
        "fn low(x:u32,y:u32)->u32{x^y} fn f(x:u32)->(u32,u32){let b=low(x^1u32);let a=low(x);(a,b)}",
    );
    ordered(
        "fn low(x:u32)->(u32,u32){(x,x)} fn f(x:u32)->((u32,u32),(u32,u32)){let a=low(x);let b=low(x^1u32);(a,b)}",
        "fn low(x:u32)->(u32,u32){(x,x)} fn f(x:u32)->((u32,u32),(u32,u32)){let b=low(x^1u32);let a=low(x);(a,b)}",
    );
}

#[test]
fn helpers_with_unsupported_execution_keep_call_order() {
    let cases = [
        (
            "extern \"C\" fn low(x:u32)->u32{x}",
            "fn f(x:u32)->(u32,u32){let a=low(x);let b=low(x^1u32);(a,b)}",
            "fn f(x:u32)->(u32,u32){let b=low(x^1u32);let a=low(x);(a,b)}",
        ),
        (
            "unsafe fn low(x:u32)->u32{x}",
            "fn f(x:u32)->(u32,u32){let a=unsafe{low(x)};let b=unsafe{low(x^1u32)};(a,b)}",
            "fn f(x:u32)->(u32,u32){let b=unsafe{low(x^1u32)};let a=unsafe{low(x)};(a,b)}",
        ),
        (
            "fn low(x:u32)->u32{let p:*const u32=&x;x}",
            "fn f(x:u32)->(u32,u32){let a=low(x);let b=low(x^1u32);(a,b)}",
            "fn f(x:u32)->(u32,u32){let b=low(x^1u32);let a=low(x);(a,b)}",
        ),
        (
            "static mut STATE: u32=0; fn low(x:u32)->u32{unsafe{STATE=x;x}}",
            "fn f(x:u32)->(u32,u32){let a=low(x);let b=low(x^1u32);(a,b)}",
            "fn f(x:u32)->(u32,u32){let b=low(x^1u32);let a=low(x);(a,b)}",
        ),
        (
            "struct D;impl Drop for D{fn drop(&mut self){}} fn low(x:u32)->u32{let d=D;x}",
            "fn f(x:u32)->(u32,u32){let a=low(x);let b=low(x^1u32);(a,b)}",
            "fn f(x:u32)->(u32,u32){let b=low(x^1u32);let a=low(x);(a,b)}",
        ),
    ];
    for (prefix, a, b) in cases {
        ordered(&format!("{prefix} {a}"), &format!("{prefix} {b}"));
    }
}

#[test]
fn unresolved_and_external_callees_keep_call_order() {
    ordered(
        "fn f(x:u32)->(u32,u32){let a=missing(x);let b=missing(x^1u32);(a,b)}",
        "fn f(x:u32)->(u32,u32){let b=missing(x^1u32);let a=missing(x);(a,b)}",
    );
    ordered(
        "fn f(x:u32)->(u32,u32){let a=core::cmp::max(x,7u32);let b=core::cmp::max(x^1u32,9u32);(a,b)}",
        "fn f(x:u32)->(u32,u32){let b=core::cmp::max(x^1u32,9u32);let a=core::cmp::max(x,7u32);(a,b)}",
    );
}

#[test]
fn dynamic_dispatch_keeps_call_order() {
    ordered(
        "fn low(x:u32)->u32{x&15u32} fn f(x:u32)->(u32,u32){let op:fn(u32)->u32=low;let a=op(x);let b=op(x^1u32);(a,b)}",
        "fn low(x:u32)->u32{x&15u32} fn f(x:u32)->(u32,u32){let op:fn(u32)->u32=low;let b=op(x^1u32);let a=op(x);(a,b)}",
    );
}

#[test]
fn callable_shadowing_keeps_the_selected_closure_effects() {
    let prefix = "fn helper(x:u32)->u32{x&15u32}";
    ordered(
        &format!(
            "{prefix} fn f(x:u32)->(u32,u32){{let helper=|v:u32|{{println!(\"{{v}}\");v}};let a=helper(x);let b=helper(x^1u32);(a,b)}}"
        ),
        &format!(
            "{prefix} fn f(x:u32)->(u32,u32){{let helper=|v:u32|{{println!(\"{{v}}\");v}};let b=helper(x^1u32);let a=helper(x);(a,b)}}"
        ),
    );
}

#[test]
fn helper_observers_and_conditional_origins_keep_call_order() {
    for attribute in [
        "#[track_caller]",
        "#[cfg(all())]",
        "#[cfg_attr(all(), inline)]",
        "#[observer]",
    ] {
        let prefix = format!("{attribute} fn helper(x:u32)->u32{{x&15u32}}");
        ordered(
            &format!(
                "{prefix} fn f(x:u32)->(u32,u32){{let a=helper(x);let b=helper(x^1u32);(a,b)}}"
            ),
            &format!(
                "{prefix} fn f(x:u32)->(u32,u32){{let b=helper(x^1u32);let a=helper(x);(a,b)}}"
            ),
        );
    }
}

#[test]
fn helpers_with_macros_or_observers_keep_call_order() {
    ordered(
        "fn low(x:u32)->u32{println!(\"{x}\");x} fn f(x:u32)->(u32,u32){let a=low(x);let b=low(x^1u32);(a,b)}",
        "fn low(x:u32)->u32{println!(\"{x}\");x} fn f(x:u32)->(u32,u32){let b=low(x^1u32);let a=low(x);(a,b)}",
    );
    ordered(
        "fn low(x:u32)->u32{observe!();x} fn f(x:u32)->(u32,u32){let a=low(x);let b=low(x^1u32);(a,b)}",
        "fn low(x:u32)->u32{observe!();x} fn f(x:u32)->(u32,u32){let b=low(x^1u32);let a=low(x);(a,b)}",
    );
}

#[test]
fn transitive_helper_chains_keep_the_scheduled_order() {
    let prefix = "fn low(x:u32)->u32{x&15u32} fn mid(x:u32)->u32{low(x)^3u32} fn high(x:u32)->u32{mid(x)|4u32}";
    scheduled(
        &format!("{prefix} fn f(x:u32)->(u32,u32){{let a=high(x);let b=low(x^1u32);(a,b)}}"),
        &format!("{prefix} fn f(x:u32)->(u32,u32){{let b=low(x^1u32);let a=high(x);(a,b)}}"),
    );
}

#[test]
fn shared_helper_diamonds_keep_the_scheduled_order() {
    let prefix = "fn base(x:u32)->u32{x&15u32} fn left(x:u32)->u32{base(x)^1u32} fn right(x:u32)->u32{base(x)|2u32}";
    scheduled(
        &format!("{prefix} fn f(x:u32)->(u32,u32){{let a=left(x);let b=right(x^1u32);(a,b)}}"),
        &format!("{prefix} fn f(x:u32)->(u32,u32){{let b=right(x^1u32);let a=left(x);(a,b)}}"),
    );
}

#[test]
fn structured_helper_bodies_keep_the_scheduled_order() {
    for body in [
        "if x<16u32{x&15u32}else{let v=x&7u32;v}",
        "{let v=x&15u32;{v^1u32}}",
        "let v=x&15u32;let w=v^3u32;w|4u32",
    ] {
        let prefix = format!("fn low(x:u32)->u32{{{body}}}");
        scheduled(
            &format!("{prefix} fn f(x:u32)->(u32,u32){{let a=low(x);let b=low(x^1u32);(a,b)}}"),
            &format!("{prefix} fn f(x:u32)->(u32,u32){{let b=low(x^1u32);let a=low(x);(a,b)}}"),
        );
    }
}

#[test]
fn boolean_helper_calls_schedule_independently() {
    scheduled(
        "fn flag(x:u32)->bool{x<16u32} fn f(x:u32)->(bool,bool){let a=flag(x);let b=flag(x^1u32);(a,b)}",
        "fn flag(x:u32)->bool{x<16u32} fn f(x:u32)->(bool,bool){let b=flag(x^1u32);let a=flag(x);(a,b)}",
    );
}

#[test]
fn multi_parameter_helpers_schedule_positionally() {
    scheduled(
        "fn pair(x:u32,y:u32)->u32{x&y} fn f(x:u32)->(u32,u32){let a=pair(x,7u32);let b=pair(x^1u32,9u32);(a,b)}",
        "fn pair(x:u32,y:u32)->u32{x&y} fn f(x:u32)->(u32,u32){let b=pair(x^1u32,9u32);let a=pair(x,7u32);(a,b)}",
    );
}

#[test]
fn renamed_helpers_keep_the_same_schedule() {
    assert_eq!(
        form("fn low(x:u32)->u32{x&15u32} fn f(input:u32)->u32{low(input)}"),
        form("fn alpha(x:u32)->u32{x&15u32} fn f(input:u32)->u32{alpha(input)}"),
        "the helper renames alpha-equivalently"
    );
}

#[test]
fn helper_calls_compose_with_the_value_folds() {
    let composed = "fn low(x:u32)->u32{x&15u32} fn f(input:u32)->(u32,u32){let b=input>>4u32;let a=low(input);(a^3u32,b)}";
    let with_dead = "fn low(x:u32)->u32{x&15u32} fn f(input:u32)->(u32,u32){let dead=42u32;let a=low(input);let b=input>>4u32;(a^3u32,b)}";
    assert_eq!(
        form(composed),
        form(with_dead),
        "the dead caller binding erases around the retained helper call"
    );
    // The call is not inlined into the caller and not merged with itself.
    distinct(
        composed,
        "fn f(input:u32)->(u32,u32){((input&15u32)^3u32,input>>4u32)}",
    );
    distinct(
        "fn low(x:u32)->u32{x&15u32} fn f(x:u32)->(u32,u32){let a=low(x);let b=low(x);(a,b)}",
        "fn low(x:u32)->u32{x&15u32} fn f(x:u32)->(u32,u32){let a=low(x);(a,a)}",
    );
}

#[test]
fn helper_totality_composes_through_nested_calls_and_scopes() {
    for body in [
        "let v=base(x);v^3u32",
        "base(base(x))|4u32",
        "(if x<16u32{let v=base(x);v}else{base(x>>1)})^3u32",
        "base({let v=x&7u32;v})",
        "x&(2u32+3u32)",
        "return x&15u32",
    ] {
        let helpers = format!("fn base(x:u32)->u32{{x&15u32}}fn low(x:u32)->u32{{{body}}}");
        scheduled(
            &format!("{helpers}fn f(x:u32)->(u32,u32){{let a=low(x);let b=base(x^1u32);(a,b)}}"),
            &format!("{helpers}fn f(x:u32)->(u32,u32){{let b=base(x^1u32);let a=low(x);(a,b)}}"),
        );
    }
}

#[test]
fn helper_future_roles_keep_joint_dependency_identity() {
    let prefix = "fn low(x:u32)->u32{x&15u32}";
    scheduled(
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let a=low(x);let b=low(x);let c=low(x^1u32);let d=low(a^c);let e=low(b^c);(d,e)}}"
        ),
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let b=low(x);let a=low(x);let c=low(x^1u32);let e=low(b^c);let d=low(a^c);(d,e)}}"
        ),
    );
    distinct(
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let a=low(x);let b=low(x);let c=low(x^1u32);let d=low(a^c);let e=low(b^c);(d,e)}}"
        ),
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let a=low(x);let b=low(x);let c=low(x^1u32);let d=low(a^c);let e=low(a^c);(d,e)}}"
        ),
    );
}

#[test]
fn incompatible_helper_argument_domains_keep_source_order() {
    for call in ["low(x,x)", "low(true)", "low()", "low::<u32>(x)"] {
        let prefix = "fn low(x:u32)->u32{x&15u32}";
        ordered(
            &format!("{prefix}fn f(x:u32)->(u32,u32){{let a={call};let b=low(x);(a,b)}}"),
            &format!("{prefix}fn f(x:u32)->(u32,u32){{let b=low(x);let a={call};(a,b)}}"),
        );
    }
}

#[test]
fn incomplete_helper_bodies_keep_source_order() {
    for body in ["", "true;", "let a=true;"] {
        let prefix = format!("fn h(x:bool)->bool{{{body}}}");
        ordered(
            &format!("{prefix}fn f(x:bool)->(bool,bool){{let a=h(x);let b=h(!x);(a,b)}}"),
            &format!("{prefix}fn f(x:bool)->(bool,bool){{let b=h(!x);let a=h(x);(a,b)}}"),
        );
    }
}

#[test]
fn helper_rejections_propagate_through_resolved_dependencies() {
    let prefix = "fn bad(x:u32)->u32{x+1u32}fn h(x:u32)->u32{bad(x)}fn k(x:u32)->u32{x>>4}";
    ordered(
        &format!("{prefix}fn f(x:u32)->(u32,u32){{let a=h(x);let b=k(x);(a,b)}}"),
        &format!("{prefix}fn f(x:u32)->(u32,u32){{let b=k(x);let a=h(x);(a,b)}}"),
    );
}

#[test]
fn helper_future_calls_keep_roles_across_an_observation_barrier() {
    let prefix = "fn low(x:u32)->u32{x&15u32}";
    scheduled(
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let a=low(x);let b=low(x);println!(\"barrier\");let c=low(x^1u32);let d=low(a^c);let e=low(b^c);(d,e)}}"
        ),
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let b=low(x);let a=low(x);println!(\"barrier\");let c=low(x^1u32);let e=low(b^c);let d=low(a^c);(d,e)}}"
        ),
    );
}

#[test]
fn tied_helper_futures_keep_repeated_current_roles() {
    let prefix = "fn low(x:u32)->u32{x&15u32}";
    scheduled(
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let a=low(x);let b=low(x);println!(\"barrier\");let c=low(x^1u32);let d=low(x^1u32);let e=low(a^c);let g=low(b^d);(e,g)}}"
        ),
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let b=low(x);let a=low(x);println!(\"barrier\");let d=low(x^1u32);let c=low(x^1u32);let g=low(b^c);let e=low(a^d);(e,g)}}"
        ),
    );
    distinct(
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let a=low(x);let b=low(x);println!(\"barrier\");let c=low(x^1u32);let d=low(x^1u32);let e=low(a^c);let g=low(b^d);(e,g)}}"
        ),
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let a=low(x);let b=low(x);println!(\"barrier\");let c=low(x^1u32);let d=low(x^1u32);let e=low(a^c);let g=low(a^d);(e,g)}}"
        ),
    );
}

#[test]
fn nested_helper_calls_in_primitive_operations_keep_their_future_roles() {
    let prefix = "fn low(x:u32)->u32{x&15u32}";
    scheduled(
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let a=low(x);let b=low(x);println!(\"barrier\");let c=low(x^1u32);let d=low(c)^a;let e=low(c)^b;(d,e)}}"
        ),
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let b=low(x);let a=low(x);println!(\"barrier\");let c=low(x^1u32);let e=low(c)^b;let d=low(c)^a;(d,e)}}"
        ),
    );
    distinct(
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let a=low(x);let b=low(x);println!(\"barrier\");let c=low(x^1u32);let d=low(c)^a;let e=low(c)^b;(d,e)}}"
        ),
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let a=low(x);let b=low(x);println!(\"barrier\");let c=low(x^1u32);let d=low(c)^a;let e=low(c)^a;(d,e)}}"
        ),
    );
}

#[test]
fn future_ordered_helper_call_arguments_keep_their_roles() {
    let prefix = "fn pair(x:u32,y:u32)->u32{x&y}fn low(x:u32)->u32{x&15u32}";
    scheduled(
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let a=low(x);let b=low(x);println!(\"barrier\");let c=low(x^1u32);let d=low(x^2u32);let e=pair(c,d);let g=a^e;(g,b)}}"
        ),
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let b=low(x);let a=low(x);println!(\"barrier\");let d=low(x^2u32);let c=low(x^1u32);let e=pair(c,d);let g=b^e;(g,a)}}"
        ),
    );
    distinct(
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let a=low(x);let b=low(x);println!(\"barrier\");let c=low(x^1u32);let d=low(x^2u32);let e=pair(c,d);let g=a^e;(g,b)}}"
        ),
        &format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let a=low(x);let b=low(x);println!(\"barrier\");let c=low(x^1u32);let d=low(x^2u32);let e=pair(d,c);let g=a^e;(g,b)}}"
        ),
    );
}

#[test]
fn attributed_helper_calls_keep_the_call_order() {
    ordered(
        "fn low(x:u32)->u32{x&15u32} fn f(x:u32)->(u32,u32){let a=x|#[cfg(all())] low(x);let b=x|#[cfg(all())] low(x^1u32);(a,b)}",
        "fn low(x:u32)->u32{x&15u32} fn f(x:u32)->(u32,u32){let b=x|#[cfg(all())] low(x^1u32);let a=x|#[cfg(all())] low(x);(a,b)}",
    );
}

#[test]
fn public_nonprimitive_helper_operands_cannot_prove_totality() {
    for operand in ["1.0f32", "'x'", "\"x\"", "b'x'"] {
        let helpers = format!("fn h(x:u32)->u32{{if true{{x}}else{{{operand}}}}}");
        ordered(
            &format!("{helpers}fn f(x:u32)->(u32,u32){{let a=h(x);let b=h(x^1u32);(a,b)}}"),
            &format!("{helpers}fn f(x:u32)->(u32,u32){{let b=h(x^1u32);let a=h(x);(a,b)}}"),
        );
    }
}

#[test]
fn inherited_generic_value_heads_cannot_select_a_function_helper() {
    ordered(
        "fn f<const C:usize>(x:u32)->(u32,u32){fn h(x:u32)->u32{C(x)}let a=h(x);let b=h(x^1u32);(a,b)}",
        "fn f<const C:usize>(x:u32)->(u32,u32){fn h(x:u32)->u32{C(x)}let b=h(x^1u32);let a=h(x);(a,b)}",
    );
}

#[test]
fn incomplete_public_item_nodes_cannot_supply_a_helper_proof() {
    let build = |body: &str| {
        let mut file: syn::File = syn::parse_str(&format!(
            "fn h(x:u32)->u32{{x}}fn f(x:u32)->(u32,u32){{{body}}}"
        ))
        .unwrap();
        let syn::Item::Fn(h) = &mut file.items[0] else {
            unreachable!()
        };
        h.block.stmts.insert(
            0,
            syn::Stmt::Item(syn::Item::Verbatim(proc_macro2::TokenStream::new())),
        );
        syn_canon::canonicalize(file)
    };
    assert_ne!(
        build("let a=h(x);let b=h(x^1u32);(a,b)"),
        build("let b=h(x^1u32);let a=h(x);(a,b)")
    );
}

#[test]
fn a_transitive_helper_can_also_be_a_caller_root() {
    let prefix = "fn outer(x:u32)->u32{inner(x)}fn inner(x:u32)->u32{x&15u32}";
    scheduled(
        &format!("{prefix}fn f(x:u32)->(u32,u32){{let a=outer(x);let b=inner(x^1u32);(a,b)}}"),
        &format!("{prefix}fn f(x:u32)->(u32,u32){{let b=inner(x^1u32);let a=outer(x);(a,b)}}"),
    );
}

#[test]
fn mixed_dependency_results_keep_the_parent_ordered() {
    let prefix = "fn bad(x:u32)->u32{x.wrapping_add(1)}fn good(x:u32)->u32{x&15u32}fn h(x:u32)->u32{bad(x)^good(x)}";
    ordered(
        &format!("{prefix}fn f(x:u32)->(u32,u32){{let a=h(x);let b=h(x^1u32);(a,b)}}"),
        &format!("{prefix}fn f(x:u32)->(u32,u32){{let b=h(x^1u32);let a=h(x);(a,b)}}"),
    );
}

#[test]
fn a_typed_branch_supplies_the_integer_literal_width() {
    let prefix = "fn h(flag:bool)->u32{let v=if flag{1}else{2u32};v}";
    scheduled(
        &format!("{prefix}fn f(flag:bool)->(u32,u32){{let a=h(flag);let b=h(!flag);(a,b)}}"),
        &format!("{prefix}fn f(flag:bool)->(u32,u32){{let b=h(!flag);let a=h(flag);(a,b)}}"),
    );
}

#[test]
fn qualified_trait_calls_keep_the_helper_ordered() {
    let prefix = "fn h(x:u32)->u32{<u32 as core::convert::From<u16>>::from(x as u16)}";
    ordered(
        &format!("{prefix}fn f(x:u32)->(u32,u32){{let a=h(x);let b=h(x^1u32);(a,b)}}"),
        &format!("{prefix}fn f(x:u32)->(u32,u32){{let b=h(x^1u32);let a=h(x);(a,b)}}"),
    );
}

#[test]
fn constructed_receiver_and_scalar_call_nodes_keep_helpers_ordered() {
    for prefix in [
        "fn h(self,x:u32)->u32{x}",
        "fn h(x:u32)->u32{x(2u32)}",
        "fn h(x:u32)->bool{1}",
    ] {
        ordered(
            &format!("{prefix}fn f(x:u32){{let a=h(x);let b=h(x^1u32);(a,b)}}"),
            &format!("{prefix}fn f(x:u32){{let b=h(x^1u32);let a=h(x);(a,b)}}"),
        );
    }
}

#[test]
fn deep_module_declarations_keep_independent_helper_summaries() {
    let open = "mod m{".repeat(65);
    let close = "}".repeat(65);
    let prefix = "fn h(x:u32)->u32{x&15u32}";
    scheduled(
        &format!("{open}{prefix}fn f(x:u32)->(u32,u32){{let a=h(x);let b=h(x^1u32);(a,b)}}{close}"),
        &format!("{open}{prefix}fn f(x:u32)->(u32,u32){{let b=h(x^1u32);let a=h(x);(a,b)}}{close}"),
    );
}

#[test]
fn bounded_local_module_routes_keep_long_calls_ordered() {
    let modules = format!(
        "{}pub fn h(x:u32)->u32{{x&15u32}}{}",
        "pub mod m{".repeat(65),
        "}".repeat(65)
    );
    let callee = format!("{}h", "m::".repeat(65));
    let build = |body: &str| format!("fn f(x:u32)->(u32,u32){{{modules}{body}}}");
    ordered(
        &build(&format!("let a={callee}(x);let b={callee}(x^1u32);(a,b)")),
        &build(&format!("let b={callee}(x^1u32);let a={callee}(x);(a,b)")),
    );
}
