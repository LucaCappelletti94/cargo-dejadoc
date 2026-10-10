//! Exhaustive small graph comparison through the public API.

use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Clone, Copy)]
enum Producer {
    Argument(usize),
    Previous(usize),
    Pair(usize, usize),
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

fn models(n: usize) -> Vec<Vec<Producer>> {
    fn extend(n: usize, prefix: &mut Vec<Producer>, out: &mut Vec<Vec<Producer>>) {
        if prefix.len() == n {
            out.push(prefix.clone());
            return;
        }
        let available = prefix.len();
        let choices = [Producer::Argument(0), Producer::Argument(1)]
            .into_iter()
            .chain((0..available).map(Producer::Previous))
            .chain((0..available).flat_map(move |left| {
                (0..available).map(move |right| Producer::Pair(left, right))
            }));
        for next in choices {
            prefix.push(next);
            extend(n, prefix, out);
            prefix.pop();
        }
    }
    let mut out = Vec::new();
    extend(n, &mut Vec::new(), &mut out);
    out
}

fn number(order: &[usize], node: usize) -> usize {
    order
        .iter()
        .position(|&candidate| candidate == node)
        .unwrap()
}

fn elements(producer: &Producer) -> (char, Vec<(u8, usize)>) {
    match producer {
        Producer::Argument(port) => ('&', vec![(0, 15), (1, *port)]),
        Producer::Previous(input) => ('^', vec![(0, 1), (2, *input)]),
        Producer::Pair(left, right) => {
            let mut elems = vec![(2, *left), (2, *right)];
            elems.sort_unstable();
            ('^', elems)
        }
    }
}

fn element_text(elem: &(u8, usize)) -> String {
    match elem.0 {
        0 => format!("l{}", elem.1),
        1 => format!("p{}", elem.1),
        _ => format!("n{}", elem.1),
    }
}

fn oracle(nodes: &[Producer], roots: [usize; 2]) -> String {
    permutations(nodes.len())
        .iter()
        .map(|perm| {
            let pos = |node: usize| {
                perm.iter()
                    .position(|&candidate| candidate == node)
                    .unwrap()
            };
            let mut text = String::new();
            for (slot, &node) in perm.iter().enumerate() {
                let mark = if slot == pos(roots[0]) && slot == pos(roots[1]) {
                    "12"
                } else if slot == pos(roots[0]) {
                    "1"
                } else if slot == pos(roots[1]) {
                    "2"
                } else {
                    ""
                };
                let (op, mut elems) = elements(&nodes[node]);
                for elem in &mut elems {
                    if elem.0 == 2 {
                        elem.1 = pos(elem.1);
                    }
                }
                elems.sort_unstable();
                let body: Vec<_> = elems.iter().map(element_text).collect();
                write!(text, "{mark}{op}{};", body.join(",")).unwrap();
            }
            write!(text, "t{},{}", pos(roots[0]), pos(roots[1])).unwrap();
            text
        })
        .min()
        .unwrap()
}

fn source(nodes: &[Producer], roots: [usize; 2], order: &[usize], raw: bool) -> String {
    let names: Vec<_> = (0..nodes.len())
        .map(|node| {
            if raw {
                ["r#type", "r#match", "r#loop"][node].to_owned()
            } else {
                format!("value{}", nodes.len() - node)
            }
        })
        .collect();
    let mut source = "fn compute(first: u32, second: u32) -> (u32, u32) {".to_owned();
    for &node in order {
        let expression = match nodes[node] {
            Producer::Argument(port) => {
                format!("{} & 15u32", ["first", "second"][port])
            }
            Producer::Previous(input) => format!("{} ^ 1u32", names[input]),
            Producer::Pair(left, right) => format!("{} ^ {}", names[left], names[right]),
        };
        write!(source, "let {} = {expression};", names[node]).unwrap();
    }
    write!(source, "({}, {}) }}", names[roots[0]], names[roots[1]]).unwrap();
    source
}

#[test]
fn all_small_legal_schedules_and_renamings_match_the_independent_graph_oracle() {
    let mut forms_by_graph = BTreeMap::<String, String>::new();
    let mut graphs_by_form = BTreeMap::<String, String>::new();
    for n in 1..=3 {
        let orders = permutations(n);
        for nodes in models(n) {
            for first in 0..n {
                for second in 0..n {
                    let roots = [first, second];
                    let graph = oracle(&nodes, roots);
                    for order in &orders {
                        if order.iter().any(|&node| match nodes[node] {
                            Producer::Previous(input) => {
                                number(order, input) >= number(order, node)
                            }
                            Producer::Pair(left, right) => {
                                number(order, left) >= number(order, node)
                                    || number(order, right) >= number(order, node)
                            }
                            Producer::Argument(_) => false,
                        }) {
                            continue;
                        }
                        for raw in [false, true] {
                            let source = source(&nodes, roots, order, raw);
                            let form = syn_canon::canonicalize(syn::parse_str(&source).unwrap())
                                .to_string();
                            if let Some(existing) = forms_by_graph.get(&graph) {
                                assert_eq!(existing, &form, "legal schedule {source}");
                            } else {
                                forms_by_graph.insert(graph.clone(), form.clone());
                            }
                            if let Some(existing) = graphs_by_form.get(&form) {
                                assert_eq!(existing, &graph, "distinct graph {source}");
                            } else {
                                graphs_by_form.insert(form, graph.clone());
                            }
                        }
                    }
                }
            }
        }
    }
}
