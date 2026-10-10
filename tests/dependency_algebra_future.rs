//! Exact identity and bounded fallback across observation regions.

use std::collections::BTreeSet;
use std::fmt::Write as _;

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

fn permutations(n: usize) -> Vec<Vec<usize>> {
    fn extend(n: usize, prefix: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if prefix.len() == n {
            out.push(prefix.clone());
            return;
        }
        for next in 0..n {
            if !prefix.contains(&next) {
                prefix.push(next);
                extend(n, prefix, out);
                prefix.pop();
            }
        }
    }
    let mut out = Vec::new();
    extend(n, &mut Vec::new(), &mut out);
    out
}

fn source(
    current: &[(&str, String)],
    future: &[(&str, String)],
    current_order: &[usize],
    future_order: &[usize],
    tail: &str,
    tuple: bool,
) -> String {
    let output = if tuple { "(u32, u32)" } else { "u32" };
    let mut text = format!("fn compute(first: u32, second: u32) -> {output} {{");
    for &index in current_order {
        let (name, init) = &current[index];
        write!(text, "let {name} = {init};").unwrap();
    }
    text.push_str("println!(\"{}\", first);");
    for &index in future_order {
        let (name, init) = &future[index];
        write!(text, "let {name} = {init};").unwrap();
    }
    format!("{text}{tail}}}")
}

fn check_family(mode: u8) -> Vec<String> {
    let mut forms = BTreeSet::new();
    let mut sources = Vec::new();
    for raw in [false, true] {
        let names = if raw {
            ["r#a", "r#b", "r#g0", "r#g1", "r#g2"]
        } else {
            ["left", "right", "next", "last", "other"]
        };
        let [a, b, f0, f1, f2] = names;
        let current = if mode <= 2 {
            [(a, "second | 3".to_owned()), (b, "second | 3".to_owned())]
        } else {
            [(a, "first & 15".to_owned()), (b, "second | 1".to_owned())]
        };
        let (future, tail, tuple) = match mode {
            0 => (
                vec![
                    (f0, "second | 5".into()),
                    (f1, "second | 6".into()),
                    (f2, "second | 7".into()),
                ],
                format!("({a} | {f0} | {f1} | {f2}, {b} ^ {f0})"),
                true,
            ),
            1 | 2 => {
                let literal = if mode == 1 { 6 } else { 5 };
                (
                    vec![
                        (f0, format!("{a} | {b} | 5")),
                        (f1, format!("{a} | {b} | {literal}")),
                    ],
                    format!("{a} | {b} | {f0} | {f1}"),
                    false,
                )
            }
            3 => (
                vec![(f0, "second | 5".into()), (f1, "second | 5".into())],
                format!("{a} ^ ({f0} | {f1}) ^ ({f0} & {f1})"),
                false,
            ),
            4 => (
                vec![
                    (f0, "second | 5".into()),
                    (f1, format!("{f0} | second")),
                    (f2, format!("{f0} | 7")),
                ],
                format!("({a} | {f1} | {f2}, {b} | {f0})"),
                true,
            ),
            _ => unreachable!(),
        };
        let future_orders = if mode == 4 {
            vec![vec![0, 1, 2], vec![0, 2, 1]]
        } else {
            permutations(future.len())
        };
        for current_order in permutations(2) {
            for future_order in &future_orders {
                let text = source(
                    &current,
                    &future,
                    &current_order,
                    future_order,
                    &tail,
                    tuple,
                );
                forms.insert(form(&text).to_string());
                sources.push(text);
            }
        }
    }
    assert_eq!(forms.len(), 1, "future family {mode}");
    sources
}

#[test]
fn identical_currents_observed_with_distinguishable_futures_keep_one_form() {
    check_family(0);
}

#[test]
fn identical_currents_feeding_futures_keep_one_form() {
    let distinct = check_family(1);
    let tied = check_family(2);
    assert_ne!(form(&distinct[0]), form(&tied[0]));
}

#[test]
fn shared_future_multiplicity_keeps_one_form() {
    let sources = check_family(3);
    let original = &sources[0];
    let shared = original
        .replace("(next | last)", "(next | next)")
        .replace("(next & last)", "(next & next)");
    assert_ne!(form(original), form(&shared));
}

#[test]
fn future_dependency_chains_keep_one_form() {
    check_family(4);
}

#[test]
fn barrier_operand_positions_distinguish_only_genuine_differences() {
    let current = [("a", "first & 15".into()), ("b", "second | 1".into())];
    let future = [("g0", "second | 5".into()), ("g1", "second | 6".into())];
    let left = source(
        &current,
        &future,
        &[0, 1],
        &[0, 1],
        "(a | (g0 & g1), b | (g1 & g0))",
        true,
    );
    let right = source(
        &current,
        &future,
        &[1, 0],
        &[1, 0],
        "(a | (g1 & g0), b | (g0 & g1))",
        true,
    );
    let different = source(
        &current,
        &future,
        &[0, 1],
        &[0, 1],
        "(b | (g0 & g1), a | (g1 & g0))",
        true,
    );
    assert_eq!(form(&left), form(&right));
    assert_ne!(form(&left), form(&different));
}

fn boundary_source(count: usize, repeated: bool) -> String {
    let init = if repeated {
        "second | second | 3"
    } else {
        "second | 3"
    };
    let current = [("a", init.into()), ("b", "second | 3".into())];
    let names: Vec<_> = (0..count).map(|index| format!("f{index}")).collect();
    let future: Vec<_> = names
        .iter()
        .map(|name| (name.as_str(), "second | 3".into()))
        .collect();
    let tail = format!("a | {}", names.join(" | "));
    source(
        &current,
        &future,
        &[0, 1],
        &(0..count).collect::<Vec<_>>(),
        &tail,
        false,
    )
}

#[test]
fn sixteen_way_future_tie_rejects_the_complete_projection() {
    assert_eq!(
        form(&boundary_source(3, true)),
        form(&boundary_source(3, false))
    );
    let source = boundary_source(16, true);
    let original = form(&source);
    let neighbor = form(&boundary_source(16, false));
    assert_ne!(original, neighbor);
    assert_eq!(original, form(&source));
    assert_eq!(
        original.body_units(),
        syn_canon::canonicalize_failing(syn::parse_str(&source).unwrap()).body_units()
    );
}

fn comparison_sources() -> [&'static str; 3] {
    [
        "fn f(first: bool, second: bool) -> (bool, bool) { let a = first | second; let b = first & second; println!(\"{}\", first); let x = a ^ b; let y = a ^ b; (a == x, y <= b) }",
        "fn g(first: bool, second: bool) -> (bool, bool) { let b = second & first; let a = second | first; println!(\"{}\", first); let y = b ^ a; let x = b ^ a; (a == x, b >= y) }",
        "fn f(first: bool, second: bool) -> (bool, bool) { let a = first | second; let b = first & second; println!(\"{}\", first); let x = a ^ b; let y = a ^ b; (b == x, y <= a) }",
    ]
}

#[test]
fn future_comparison_paths_retain_ordered_roles() {
    let [original, reordered, different] = comparison_sources();
    assert_eq!(form(original), form(reordered));
    assert_ne!(form(original), form(different));
}

#[test]
fn identical_currents_keep_their_indirect_future_output_roles() {
    let original = "fn f(input: u32) -> (u32, u32) { let a = input | 3; let b = input | 3; println!(\"barrier\"); let shared = input | 5; let x = a ^ shared; let y = b ^ shared; (x, y) }";
    for reordered in [
        "fn f(input: u32) -> (u32, u32) { let b = input | 3; let a = input | 3; println!(\"barrier\"); let shared = input | 5; let x = a ^ shared; let y = b ^ shared; (x, y) }",
        "fn f(input: u32) -> (u32, u32) { let a = input | 3; let b = input | 3; println!(\"barrier\"); let shared = input | 5; let y = b ^ shared; let x = a ^ shared; (x, y) }",
        "fn f(input: u32) -> (u32, u32) { let b = input | 3; let a = input | 3; println!(\"barrier\"); let shared = input | 5; let y = b ^ shared; let x = a ^ shared; (x, y) }",
    ] {
        assert_eq!(form(original), form(reordered));
    }
}

#[test]
fn tied_future_comparison_definitions_keep_operand_roles() {
    let original = "fn f(first: u32, second: u32) -> (bool, bool) { let a = first & 7; let b = second | 9; println!(\"{}\", first); let c = a < b; let d = a < b; (c & d, c | d) }";
    let reordered = "fn g(first: u32, second: u32) -> (bool, bool) { let b = second | 9; let a = first & 7; println!(\"{}\", first); let d = b > a; let c = b > a; (d & c, d | c) }";
    let different = "fn f(first: u32, second: u32) -> (bool, bool) { let a = first & 7; let b = second | 9; println!(\"{}\", first); let c = b < a; let d = a < b; (c & d, c | d) }";
    assert_eq!(form(original), form(reordered));
    assert_ne!(form(original), form(different));
}
