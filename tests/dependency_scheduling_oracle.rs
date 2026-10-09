//! Exhaustive small ordered-graph comparison through the public API.

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

fn oracle(nodes: &[Producer], roots: [usize; 2], orders: &[Vec<usize>]) -> String {
    orders
        .iter()
        .map(|order| {
            let rows: Vec<_> = order
                .iter()
                .map(|&node| match nodes[node] {
                    Producer::Argument(port) => format!("and:u32:arg{port}:15u32"),
                    Producer::Previous(input) => {
                        format!("xor:u32:node{}:1u32", number(order, input))
                    }
                    Producer::Pair(left, right) => {
                        format!(
                            "xor:u32:node{}:node{}",
                            number(order, left),
                            number(order, right)
                        )
                    }
                })
                .collect();
            format!(
                "{rows:?}:{}:{}",
                number(order, roots[0]),
                number(order, roots[1])
            )
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
                    let graph = oracle(&nodes, roots, &orders);
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
