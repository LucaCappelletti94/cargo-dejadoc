//! Exhaustive small unordered-graph algebra comparison through the public API.

use std::collections::BTreeMap;
use std::fmt::Write as _;

const OPS: [char; 3] = ['&', '|', '^'];

#[derive(Clone, Copy)]
enum Producer {
    Argument(usize),
    XorOne(usize),
    OrOne(usize),
    Flat(char, usize, usize),
    Left(char, usize, usize, usize),
    Right(char, usize, usize, usize),
    LitLeft(char, usize, u8, u8),
    LitRight(char, usize, u8, u8),
    Copy(usize),
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

fn choices(available: usize) -> Vec<Producer> {
    let mut list = vec![Producer::Argument(0), Producer::Argument(1)];
    for index in 0..available {
        list.push(Producer::XorOne(index));
        list.push(Producer::OrOne(index));
        list.push(Producer::Copy(index));
    }
    for left in 0..available {
        for right in 0..available {
            for op in OPS {
                list.push(Producer::Flat(op, left, right));
            }
        }
    }
    for a in 0..available {
        for b in 0..available {
            for c in 0..available {
                for op in OPS {
                    list.push(Producer::Left(op, a, b, c));
                    list.push(Producer::Right(op, a, b, c));
                }
            }
        }
    }
    for index in 0..available {
        for op in OPS {
            for &(first, second) in literal_pairs(op) {
                list.push(Producer::LitLeft(op, index, first, second));
                list.push(Producer::LitRight(op, index, first, second));
            }
        }
    }
    list
}

fn literal_pairs(op: char) -> &'static [(u8, u8)] {
    match op {
        '&' => &[(15, 15)],
        '|' => &[(1, 1)],
        _ => &[(1, 1), (1, 2)],
    }
}

fn models(n: usize) -> Vec<Vec<Producer>> {
    fn extend(n: usize, prefix: &mut Vec<Producer>, out: &mut Vec<Vec<Producer>>) {
        if prefix.len() == n {
            out.push(prefix.clone());
            return;
        }
        for next in choices(prefix.len()) {
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

fn references(producer: &Producer) -> Vec<usize> {
    match producer {
        Producer::Argument(_) => Vec::new(),
        Producer::XorOne(input) | Producer::OrOne(input) | Producer::Copy(input) => vec![*input],
        Producer::Flat(_, left, right) => vec![*left, *right],
        Producer::Left(_, a, b, c) | Producer::Right(_, a, b, c) => vec![*a, *b, *c],
        Producer::LitLeft(_, index, _, _) | Producer::LitRight(_, index, _, _) => vec![*index],
    }
}

/// The canonical operand collection of a declaration after the explicit
/// bitwise laws, kind zero for a bare value.
fn collection(producer: &Producer) -> (u8, Vec<(u8, usize)>) {
    match producer {
        Producer::Argument(port) => (1, vec![(0, 15), (1, *port)]),
        Producer::XorOne(input) => (3, vec![(0, 1), (2, *input)]),
        Producer::OrOne(input) => (2, vec![(0, 1), (2, *input)]),
        Producer::Copy(input) => (0, vec![(2, *input)]),
        Producer::Flat(op, left, right) => bitwise(*op, vec![(2, *left), (2, *right)]),
        Producer::Left(op, a, b, c) | Producer::Right(op, a, b, c) => {
            bitwise(*op, vec![(2, *a), (2, *b), (2, *c)])
        }
        Producer::LitLeft(op, index, first, second)
        | Producer::LitRight(op, index, first, second) => bitwise(
            *op,
            vec![
                (2, *index),
                (0, usize::from(*first)),
                (0, usize::from(*second)),
            ],
        ),
    }
}

fn bitwise(op: char, mut elems: Vec<(u8, usize)>) -> (u8, Vec<(u8, usize)>) {
    let kind: u8 = match op {
        '&' => 1,
        '|' => 2,
        _ => 3,
    };
    elems.sort_unstable();
    if kind == 3 {
        let literals: Vec<usize> = elems
            .iter()
            .filter(|elem| elem.0 == 0)
            .map(|elem| elem.1)
            .collect();
        if literals.len() == 2 {
            elems.retain(|elem| elem.0 != 0);
            elems.push((0, literals[0] ^ literals[1]));
            elems.sort_unstable();
        }
    } else {
        elems.dedup();
        if elems.len() == 1 {
            return (0, elems);
        }
    }
    (kind, elems)
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
                let (kind, mut elems) = collection(&nodes[node]);
                for elem in &mut elems {
                    if elem.0 == 2 {
                        elem.1 = pos(elem.1);
                    }
                }
                elems.sort_unstable();
                let body: Vec<_> = elems.iter().map(element_text).collect();
                write!(text, "{mark}{kind}{};", body.join(",")).unwrap();
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
    let mut text = "fn compute(first: u32, second: u32) -> (u32, u32) {".to_owned();
    for &node in order {
        let expression = match nodes[node] {
            Producer::Argument(port) => {
                format!("{} & 15u32", ["first", "second"][port])
            }
            Producer::XorOne(input) => format!("{} ^ 1u32", names[input]),
            Producer::OrOne(input) => format!("{} | 1u32", names[input]),
            Producer::Flat(op, left, right) => {
                format!("{} {op} {}", names[left], names[right])
            }
            Producer::Left(op, a, b, c) => {
                format!("({} {op} {}) {op} {}", names[a], names[b], names[c])
            }
            Producer::Right(op, a, b, c) => {
                format!("{} {op} ({} {op} {})", names[a], names[b], names[c])
            }
            Producer::LitLeft(op, index, first, second) => {
                format!("({} {op} {first}u32) {op} {second}u32", names[index])
            }
            Producer::LitRight(op, index, first, second) => {
                format!("{} {op} ({first}u32 {op} {second}u32)", names[index])
            }
            Producer::Copy(input) => names[input].clone(),
        };
        write!(text, "let {} = {expression};", names[node]).unwrap();
    }
    write!(text, "({}, {})}}", names[roots[0]], names[roots[1]]).unwrap();
    text
}

#[test]
fn all_small_algebra_graphs_match_the_independent_unordered_oracle() {
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
                        if order.iter().any(|&node| {
                            references(&nodes[node])
                                .iter()
                                .any(|&input| number(order, input) >= number(order, node))
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

#[derive(Clone, Copy)]
enum Operand {
    Param(u8),
    Lit(u32),
    Node(usize),
}

#[derive(Clone, Copy)]
struct Cell {
    region: u8,
    op: char,
    lhs: Operand,
    rhs: Operand,
}

const C0: Cell = Cell {
    region: 0,
    op: '&',
    lhs: Operand::Param(0),
    rhs: Operand::Lit(15),
};
const C1: Cell = Cell {
    region: 0,
    op: '|',
    lhs: Operand::Param(1),
    rhs: Operand::Lit(1),
};
const C2: Cell = Cell {
    region: 1,
    op: '^',
    lhs: Operand::Node(0),
    rhs: Operand::Lit(3),
};
const C3: Cell = Cell {
    region: 1,
    op: '^',
    lhs: Operand::Node(2),
    rhs: Operand::Node(0),
};
const C4: Cell = Cell {
    region: 1,
    op: '|',
    lhs: Operand::Node(2),
    rhs: Operand::Node(0),
};

fn flip(cell: Cell) -> Cell {
    Cell {
        lhs: cell.rhs,
        rhs: cell.lhs,
        ..cell
    }
}

fn operand_element(operand: &Operand) -> (u8, usize) {
    match operand {
        Operand::Param(port) => (1, usize::from(*port)),
        Operand::Lit(value) => (
            0,
            usize::try_from(*value).expect("the fixture literal fits usize"),
        ),
        Operand::Node(index) => (2, *index),
    }
}

fn cell_elements(cell: &Cell) -> (u8, Vec<(u8, usize)>) {
    let kind: u8 = match cell.op {
        '&' => 1,
        '|' => 2,
        _ => 3,
    };
    let mut elems = vec![operand_element(&cell.lhs), operand_element(&cell.rhs)];
    elems.sort_unstable();
    if kind != 3 {
        elems.dedup();
        if elems.len() == 1 {
            return (0, elems);
        }
    }
    (kind, elems)
}

fn cross_graph(cells: &[Cell], obs: Option<[usize; 2]>, roots: [usize; 2]) -> String {
    permutations(cells.len())
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
                let (kind, mut elems) = cell_elements(&cells[node]);
                for elem in &mut elems {
                    if elem.0 == 2 {
                        elem.1 = pos(elem.1);
                    }
                }
                elems.sort_unstable();
                let body: Vec<_> = elems.iter().map(element_text).collect();
                write!(
                    text,
                    "g{}{}{}{};",
                    cells[node].region,
                    mark,
                    kind,
                    body.join(",")
                )
                .unwrap();
            }
            if let Some(obs) = obs {
                write!(text, "o{},{} ", pos(obs[0]), pos(obs[1])).unwrap();
            }
            write!(text, "t{},{}", pos(roots[0]), pos(roots[1])).unwrap();
            text
        })
        .min()
        .unwrap()
}

fn operand_text(operand: &Operand, names: &[String]) -> String {
    match operand {
        Operand::Param(port) => ["first", "second"][usize::from(*port)].to_owned(),
        Operand::Lit(value) => format!("{value}u32"),
        Operand::Node(index) => names[*index].clone(),
    }
}

fn cross_source(
    cells: &[Cell],
    order0: &[usize],
    order1: &[usize],
    obs: Option<[usize; 2]>,
    roots: [usize; 2],
    names: &[String],
) -> String {
    let mut text = "fn compute(first: u32, second: u32) -> (u32, u32) {".to_owned();
    for &index in order0 {
        let cell = &cells[index];
        write!(
            text,
            "let {} = {} {} {};",
            names[index],
            operand_text(&cell.lhs, names),
            cell.op,
            operand_text(&cell.rhs, names)
        )
        .unwrap();
    }
    if let Some(obs) = obs {
        write!(
            text,
            "println!(\"{{}} {{}}\", {}, {});",
            names[obs[0]], names[obs[1]]
        )
        .unwrap();
    } else {
        text.push_str("println!(\"barrier\");");
    }
    for &index in order1 {
        let cell = &cells[index];
        write!(
            text,
            "let {} = {} {} {};",
            names[index],
            operand_text(&cell.lhs, names),
            cell.op,
            operand_text(&cell.rhs, names)
        )
        .unwrap();
    }
    write!(text, "({}, {})}}", names[roots[0]], names[roots[1]]).unwrap();
    text
}

type Variant<'a> = (
    &'a [Cell],
    &'a [usize],
    &'a [usize],
    [usize; 2],
    [usize; 2],
    &'a [String],
);
type Shape<'a> = (&'a [Cell], &'a [usize], &'a [usize], [usize; 2], [usize; 2]);

const REPEATED_XOR: [Cell; 5] = [
    C0,
    C1,
    C2,
    Cell {
        rhs: Operand::Node(2),
        ..C3
    },
    C4,
];
const OR_DEFINITION: [Cell; 5] = [C0, C1, Cell { op: '|', ..C2 }, C3, C4];
const DUPLICATE_DEFINITION: [Cell; 6] = [
    C0,
    C1,
    C2,
    C2,
    Cell {
        lhs: Operand::Node(3),
        ..C3
    },
    C4,
];
const XOR_SECOND: [Cell; 5] = [C0, C1, C2, C3, Cell { op: '^', ..C4 }];
const REPEATED_VALUE: [Cell; 5] = [
    C0,
    C1,
    C2,
    Cell {
        lhs: Operand::Node(0),
        rhs: Operand::Node(0),
        ..C3
    },
    C4,
];
const COLLAPSED_VALUE: [Cell; 5] = [
    C0,
    C1,
    C2,
    Cell {
        op: '&',
        lhs: Operand::Node(0),
        rhs: Operand::Node(0),
        ..C3
    },
    C4,
];

#[test]
fn cross_region_future_sharing_ties_keep_exact_relations() {
    let names: Vec<String> = (0..6).map(|index| format!("value{}", 6 - index)).collect();
    let raw: Vec<String> = [
        "observed_left",
        "observed_right",
        "r#loop",
        "r#move",
        "r#ref",
        "r#box",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let shifted: Vec<String> = (0..6)
        .map(|index| format!("value{}", (index + 3) % 6 + 1))
        .collect();

    let base: [Cell; 5] = [C0, C1, C2, C3, C4];
    let commuted_operands: [Cell; 5] = [C0, C1, C2, flip(C3), flip(C4)];
    let commuted_literal: [Cell; 5] = [C0, C1, flip(C2), C3, C4];
    let commuted_right: [Cell; 5] = [C0, flip(C1), C2, C3, C4];

    let equivalent: [Variant<'_>; 8] = [
        (&base, &[0, 1], &[2, 3, 4], [0, 1], [3, 4], &names),
        (&base, &[1, 0], &[2, 3, 4], [0, 1], [3, 4], &names),
        (&base, &[0, 1], &[2, 4, 3], [0, 1], [3, 4], &names),
        (
            &commuted_operands,
            &[0, 1],
            &[2, 3, 4],
            [0, 1],
            [3, 4],
            &names,
        ),
        (
            &commuted_literal,
            &[0, 1],
            &[2, 3, 4],
            [0, 1],
            [3, 4],
            &names,
        ),
        (&commuted_right, &[0, 1], &[2, 3, 4], [0, 1], [3, 4], &names),
        (&base, &[0, 1], &[2, 3, 4], [0, 1], [3, 4], &raw),
        (&base, &[0, 1], &[2, 3, 4], [0, 1], [3, 4], &shifted),
    ];

    let separated: [Shape<'_>; 8] = [
        // The observation argument order.
        (&base, &[0, 1], &[2, 3, 4], [1, 0], [3, 4]),
        // The ordered output positions.
        (&base, &[0, 1], &[2, 3, 4], [0, 1], [4, 3]),
        // XOR multiplicity of a repeated value.
        (&REPEATED_XOR, &[0, 1], &[2, 3, 4], [0, 1], [3, 4]),
        // The future definition operator.
        (&OR_DEFINITION, &[0, 1], &[2, 3, 4], [0, 1], [3, 4]),
        // A duplicated future definition keeps two declarations.
        (
            &DUPLICATE_DEFINITION,
            &[0, 1],
            &[2, 3, 4, 5],
            [0, 1],
            [4, 5],
        ),
        // The second consumer operator.
        (&XOR_SECOND, &[0, 1], &[2, 3, 4], [0, 1], [3, 4]),
        // A repeated value in the second consumer.
        (&REPEATED_VALUE, &[0, 1], &[2, 3, 4], [0, 1], [3, 4]),
        // A12 collapse of the repeated value in the second consumer.
        (&COLLAPSED_VALUE, &[0, 1], &[2, 3, 4], [0, 1], [3, 4]),
    ];

    let mut forms_by_graph = BTreeMap::<String, Vec<String>>::new();
    for (cells, order0, order1, obs, roots, names) in equivalent {
        let source = cross_source(cells, order0, order1, Some(obs), roots, names);
        let form = syn_canon::canonicalize(syn::parse_str(&source).unwrap()).to_string();
        forms_by_graph
            .entry(cross_graph(cells, Some(obs), roots))
            .or_default()
            .push(form);
    }
    for (cells, order0, order1, obs, roots) in separated {
        let source = cross_source(cells, order0, order1, Some(obs), roots, &names);
        let form = syn_canon::canonicalize(syn::parse_str(&source).unwrap()).to_string();
        forms_by_graph
            .entry(cross_graph(cells, Some(obs), roots))
            .or_default()
            .push(form);
    }
    for (graph, forms) in &forms_by_graph {
        for form in &forms[1..] {
            assert_eq!(&forms[0], form, "equivalent graph {graph}");
        }
    }
    for (left_graph, left_forms) in &forms_by_graph {
        for (right_graph, right_forms) in &forms_by_graph {
            if left_graph != right_graph {
                assert_ne!(
                    left_forms[0], right_forms[0],
                    "distinct graphs {left_graph} {right_graph}"
                );
            }
        }
    }
}

#[test]
fn indirect_future_output_and_shared_origins_match_the_exact_oracle() {
    let shared = Cell {
        region: 1,
        op: '|',
        lhs: Operand::Param(1),
        rhs: Operand::Lit(5),
    };
    let consumer = |current, future| Cell {
        region: 1,
        op: '^',
        lhs: Operand::Node(current),
        rhs: Operand::Node(future),
    };
    let mut forms_by_graph = BTreeMap::<String, String>::new();
    let mut graphs_by_form = BTreeMap::<String, String>::new();
    for separate in [false, true] {
        let cells = [
            C0,
            C0,
            shared,
            shared,
            consumer(0, 2),
            consumer(1, if separate { 3 } else { 2 }),
        ];
        for roots in [[4, 5], [5, 4]] {
            let graph = cross_graph(&cells, None, roots);
            for raw in [false, true] {
                let names: Vec<_> = (0..6)
                    .map(|index| {
                        if raw {
                            format!("r#value{index}")
                        } else {
                            format!("node{index}")
                        }
                    })
                    .collect();
                for current_order in [[0, 1], [1, 0]] {
                    for order in permutations(4) {
                        let future_order: Vec<_> =
                            order.into_iter().map(|index| index + 2).collect();
                        let position = |node| {
                            future_order
                                .iter()
                                .position(|&candidate| candidate == node)
                                .unwrap()
                        };
                        if future_order.iter().any(|&node| [cells[node].lhs, cells[node].rhs].iter().any(|input| matches!(input, Operand::Node(parent) if *parent >= 2 && position(*parent) >= position(node)))) {
                            continue;
                        }
                        let text = cross_source(
                            &cells,
                            &current_order,
                            &future_order,
                            None,
                            roots,
                            &names,
                        );
                        let form =
                            syn_canon::canonicalize(syn::parse_str(&text).unwrap()).to_string();
                        if let Some(existing) = forms_by_graph.get(&graph) {
                            assert_eq!(existing, &form, "schedule {text}");
                        } else {
                            forms_by_graph.insert(graph.clone(), form.clone());
                        }
                        if let Some(existing) = graphs_by_form.get(&form) {
                            assert_eq!(existing, &graph, "sharing {text}");
                        } else {
                            graphs_by_form.insert(form, graph.clone());
                        }
                    }
                }
            }
        }
    }
}
