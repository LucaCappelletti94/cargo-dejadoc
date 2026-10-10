//! Future boolean graphs preserve exact sharing and ordered roots.

use std::collections::BTreeSet;

use dejadoc::{DocTest, Report, group};

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

fn failing(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize_failing(syn::parse_str(source).unwrap())
}

fn same(left: &str, right: &str) {
    let a = form(left);
    let b = form(right);
    assert_eq!(a, b, "{left}\n{right}");
    assert_eq!(a.body_units(), b.body_units());
    assert_eq!(a.leaf_tokens(), b.leaf_tokens());
    assert_ne!(failing(left), failing(right));
}

fn separate(left: &str, right: &str) {
    assert_ne!(form(left), form(right), "{left}\n{right}");
    assert_ne!(failing(left), failing(right));
}

fn source(current: [usize; 2], future: [usize; 4], tail: &str) -> String {
    let current_statements = ["let a=p;", "let b=q;"];
    let future_statements = ["let x=a;", "let w=a;", "let y=!x;", "let z=!w;"];
    format!(
        "fn f(p:bool,q:bool)->(bool,bool,bool){{{c0}{c1}println!(\"barrier\");{f0}{f1}{f2}{f3}{tail}}}",
        c0 = current_statements[current[0]],
        c1 = current_statements[current[1]],
        f0 = future_statements[future[0]],
        f1 = future_statements[future[1]],
        f2 = future_statements[future[2]],
        f3 = future_statements[future[3]],
        tail = tail
    )
}

/// The legal orders of x, w, y, z with y after x and z after w.
fn future_orders() -> [[usize; 4]; 6] {
    [
        [0, 1, 2, 3],
        [0, 1, 3, 2],
        [0, 2, 1, 3],
        [1, 0, 2, 3],
        [1, 0, 3, 2],
        [1, 3, 0, 2],
    ]
}

fn family_sources(tail: &str) -> Vec<String> {
    let mut sources = Vec::new();
    for current in [[0usize, 1], [1, 0]] {
        for future in future_orders() {
            sources.push(source(current, future, tail));
        }
    }
    sources
}

const SHARED_ORIGIN: &str = "fn f(p:bool,q:bool)->(bool,bool,bool){let a=p;let b=q;println!(\"barrier\");let x=a;let w=a;let y=!x;let z=!x;(y,z,b)}";

#[test]
fn tied_future_negations_keep_one_form_across_permuted_regions() {
    let sources = family_sources("(y,z,b)");
    let original = &sources[0];
    let mut forms = BTreeSet::new();
    for source in &sources {
        forms.insert(form(source).to_string());
    }
    assert_eq!(forms.len(), 1, "{original}");
    for source in &sources[1..] {
        same(original, source);
    }
}

#[test]
fn tied_future_negations_keep_shared_origin_and_swapped_roots_separate() {
    let original = source([0, 1], [0, 1, 2, 3], "(y,z,b)");
    let swapped = source([0, 1], [0, 1, 2, 3], "(b,z,y)");
    separate(&original, SHARED_ORIGIN);
    separate(&original, &swapped);
    separate(SHARED_ORIGIN, &swapped);
}

fn site(item: &str, code: &str) -> DocTest {
    DocTest {
        file: format!("src/{item}.rs"),
        line: 7,
        end: Some(11),
        item: item.to_owned(),
        info: Vec::new(),
        code: code.to_owned(),
        allow: false,
        self_type: None,
        public: false,
    }
}

fn members(report: &Report) -> Vec<Vec<&str>> {
    let mut groups: Vec<Vec<&str>> = report
        .groups
        .iter()
        .map(|group| {
            let mut items: Vec<_> = group.sites.iter().map(|site| site.item.as_str()).collect();
            items.sort_unstable();
            items
        })
        .collect();
    groups.sort_unstable();
    groups
}

#[test]
fn tied_future_negation_grouping_keeps_the_exact_family_members() {
    let mut sites = family_sources("(y,z,b)")
        .into_iter()
        .enumerate()
        .map(|(index, code)| site(&format!("fam_{index:02}"), &code))
        .collect::<Vec<_>>();
    sites.push(site("shared_origin", SHARED_ORIGIN));
    sites.push(site(
        "swapped_roots",
        &source([0, 1], [0, 1, 2, 3], "(b,z,y)"),
    ));
    let report = group(&sites, 1, 0);
    assert_eq!(
        members(&report),
        [
            vec![
                "fam_00", "fam_01", "fam_02", "fam_03", "fam_04", "fam_05", "fam_06", "fam_07",
                "fam_08", "fam_09", "fam_10", "fam_11"
            ],
            vec!["shared_origin"],
            vec!["swapped_roots"]
        ]
    );
}

#[test]
fn inverted_future_conditions_preserve_origins_branches_and_output_positions() {
    let original = "fn f(p:bool,q:u8,r:u8)->(u8,u8){let a=p;let b=p;println!(\"barrier\");let c=if !a{q}else{r};let d=if b{3u8}else{4u8};(c,d)}";
    let oriented = "fn g(p:bool,q:u8,r:u8)->(u8,u8){let a=p;let b=p;println!(\"barrier\");let c=if a{r}else{q};let d=if b{3u8}else{4u8};(c,d)}";
    let permuted = "fn h(p:bool,q:u8,r:u8)->(u8,u8){let b=p;let a=p;println!(\"barrier\");let c=if a{r}else{q};let d=if b{3u8}else{4u8};(c,d)}";
    let shared = "fn f(p:bool,q:u8,r:u8)->(u8,u8){let a=p;let b=p;println!(\"barrier\");let c=if a{r}else{q};let d=if a{3u8}else{4u8};(c,d)}";
    let branches = "fn f(p:bool,q:u8,r:u8)->(u8,u8){let a=p;let b=p;println!(\"barrier\");let c=if a{q}else{r};let d=if b{3u8}else{4u8};(c,d)}";
    let outputs = "fn f(p:bool,q:u8,r:u8)->(u8,u8){let a=p;let b=p;println!(\"barrier\");let c=if a{r}else{q};let d=if b{3u8}else{4u8};(d,c)}";
    for equivalent in [oriented, permuted] {
        same(original, equivalent);
    }
    for neighbor in [shared, branches, outputs] {
        separate(original, neighbor);
    }
    let sites = [
        ("original", original),
        ("oriented", oriented),
        ("permuted", permuted),
        ("shared", shared),
        ("branches", branches),
        ("outputs", outputs),
    ]
    .map(|(item, code)| site(item, code));
    assert_eq!(
        members(&group(&sites, 1, 0)),
        [
            vec!["branches"],
            vec!["oriented", "original", "permuted"],
            vec!["outputs"],
            vec!["shared"]
        ]
    );
}
