//! Structure grouping membership, canonical sizes and thresholds.

#[derive(serde::Deserialize)]
struct SourcePair {
    id: String,
    a: String,
    b: String,
}

use dejadoc::{Dejadoc, DocTest, Kind, Remedy, Report, SourceFile, TargetScan, group};

fn site(item: &str, code: &str, info: &[&str]) -> DocTest {
    DocTest {
        file: format!("src/{item}.rs"),
        line: 7,
        end: Some(11),
        item: item.to_owned(),
        info: info.iter().map(|tag| (*tag).to_owned()).collect(),
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

fn function_ids(report: &Report) -> Vec<&str> {
    let mut ids: Vec<&str> = report
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Function)
        .map(|group| group.id.as_str())
        .collect();
    ids.sort_unstable();
    ids
}

fn target(text: &str) -> TargetScan {
    TargetScan {
        name: "structure".to_owned(),
        files: vec![SourceFile {
            path: "src/lib.rs".to_owned(),
            segments: Vec::new(),
            parsed: syn::parse_file(text).unwrap(),
            text: text.to_owned(),
            rustdoc: true,
        }],
        library: true,
    }
}

#[test]
fn structure_cli_fixture_preserves_exact_function_and_doctest_members() {
    let text = include_str!("fixtures/structurews/src/lib.rs");
    let target = target(text);
    let report = Dejadoc::default().functions().fn_min_tokens(0).run_targets(
        "",
        std::slice::from_ref(&target),
        &|_, _| None,
    );
    let function_groups: Vec<_> = report
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Function)
        .collect();
    let mut function_members: Vec<Vec<_>> = function_groups
        .iter()
        .map(|group| {
            let mut items: Vec<_> = group.sites.iter().map(|site| site.item.as_str()).collect();
            items.sort_unstable();
            items
        })
        .collect();
    function_members.sort_unstable();
    assert_eq!(
        function_members,
        [
            vec!["structure::arms::first", "structure::arms::same"],
            vec!["structure::fields::first", "structure::fields::same"],
        ]
    );
    for group in &function_groups {
        assert_eq!(group.remedy, Some(Remedy::Delete));
        assert!(group.sites.iter().all(|site| {
            site.file == "src/lib.rs" && site.line > 0 && site.end.is_some() && site.public
        }));
    }
    let doctest_groups: Vec<_> = report
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Doctest)
        .collect();
    assert_eq!(doctest_groups.len(), 1);
    let sites = &doctest_groups[0].sites;
    assert_eq!(sites.len(), 2);
    assert!(sites.iter().all(|site| {
        site.item == "structure" && site.info.is_empty() && site.file == "src/lib.rs"
    }));
    assert_ne!(sites[0].line, sites[1].line);
    assert!(doctest_groups[0].remedy.is_none());
    let grouped: Vec<&DocTest> = report
        .groups
        .iter()
        .flat_map(|group| group.sites.iter())
        .collect();
    assert_eq!(
        grouped
            .iter()
            .filter(|site| site.item == "structure")
            .count(),
        2
    );
    assert!(
        !grouped
            .iter()
            .any(|site| { site.info.iter().any(|tag| tag == "should_panic") })
    );
}

#[test]
fn structure_cli_fixture_boundaries_and_reversed_sites() {
    let text = include_str!("fixtures/structurews/src/lib.rs");
    let mut target = target(text);
    let base = Dejadoc::default().functions().fn_min_tokens(0).run_targets(
        "",
        std::slice::from_ref(&target),
        &|_, _| None,
    );
    let function_groups: Vec<_> = base
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Function)
        .collect();
    assert_eq!(function_groups.len(), 2);
    for group in &function_groups {
        let prefixes: Vec<&str> = group
            .sites
            .iter()
            .map(|site| {
                let item = site.item.as_str();
                &item[..item.rfind("::").unwrap()]
            })
            .collect();
        assert!(
            prefixes.iter().all(|prefix| *prefix == prefixes[0]),
            "one module per group"
        );
    }
    target.files[0].parsed.items.reverse();
    let reversed_scan = Dejadoc::default().functions().fn_min_tokens(0).run_targets(
        "",
        std::slice::from_ref(&target),
        &|_, _| None,
    );
    assert_eq!(function_ids(&base), function_ids(&reversed_scan));
    let doctest_sites: Vec<DocTest> = base
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Doctest)
        .flat_map(|group| group.sites.iter().cloned())
        .collect();
    assert_eq!(doctest_sites.len(), 2);
    assert!(doctest_sites.iter().all(|site| site.info.is_empty()));
    let reversed_sites: Vec<DocTest> = doctest_sites.iter().rev().cloned().collect();
    let included = group(&doctest_sites, 1, 0);
    let reversed_doctests = group(&reversed_sites, 1, 0);
    assert_eq!(included.groups.len(), 1);
    assert_eq!(members(&included), members(&reversed_doctests));
    assert_eq!(included.groups[0].id, reversed_doctests.groups[0].id);
}

fn find_function<'a>(file: &'a syn::File, module: &str, name: &str) -> &'a syn::ItemFn {
    for item in &file.items {
        if let syn::Item::Mod(mod_) = item
            && mod_.ident == module
            && let Some((_, items)) = &mod_.content
        {
            for inner in items {
                if let syn::Item::Fn(function) = inner
                    && function.sig.ident == name
                {
                    return function;
                }
            }
        }
    }
    panic!("fixture is missing {module}::{name}");
}

fn doc_blocks(text: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for raw in text.lines() {
        let line = raw.strip_prefix("//!").unwrap_or(raw);
        if line.trim_start().starts_with("```") {
            if let Some(block) = current.take() {
                blocks.push(block);
            } else {
                current = Some(String::new());
            }
        } else if let Some(block) = current.as_mut() {
            block.push_str(line);
            block.push('\n');
        }
    }
    blocks
}

#[test]
fn structure_threshold_uses_public_canonical_sizes() {
    let text = include_str!("fixtures/structurews/src/lib.rs");
    let file = syn::parse_file(text).unwrap();
    let entries: [(&[String], &syn::File); 1] = [(&[], &file)];
    let context = syn_canon::SourceContext::new(entries);
    let target = target(text);
    let base = Dejadoc::default().functions().fn_min_tokens(0).run_targets(
        "",
        std::slice::from_ref(&target),
        &|_, _| None,
    );
    let function_groups: Vec<_> = base
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Function)
        .collect();
    assert_eq!(function_groups.len(), 2);
    for group in &function_groups {
        for site in &group.sites {
            let item = site.item.as_str();
            let (module, name) = item
                .split_once("::")
                .and_then(|(_, rest)| rest.rsplit_once("::"))
                .unwrap();
            let function = find_function(&file, module, name);
            let size = context
                .function(&function.sig, &function.block)
                .expect("the site is indexed")
                .canonicalize()
                .body_units();
            assert_eq!(group.tokens, size, "{item}");
        }
        let floor = group.tokens;
        let included = Dejadoc::default()
            .functions()
            .fn_min_tokens(floor)
            .run_targets("", std::slice::from_ref(&target), &|_, _| None);
        let excluded = Dejadoc::default()
            .functions()
            .fn_min_tokens(floor + 1)
            .run_targets("", std::slice::from_ref(&target), &|_, _| None);
        assert!(included.groups.iter().any(|member| member.id == group.id));
        assert!(!excluded.groups.iter().any(|member| member.id == group.id));
    }
    let blocks = doc_blocks(text);
    assert_eq!(blocks.len(), 4);
    let first_form = syn_canon::canonicalize(syn::parse_str::<syn::File>(&blocks[0]).unwrap());
    let second_form = syn_canon::canonicalize(syn::parse_str::<syn::File>(&blocks[1]).unwrap());
    let third_form = syn_canon::canonicalize(syn::parse_str::<syn::File>(&blocks[2]).unwrap());
    let fourth_form = syn_canon::canonicalize(syn::parse_str::<syn::File>(&blocks[3]).unwrap());
    assert_eq!(first_form, second_form);
    assert_ne!(first_form, third_form);
    assert_ne!(second_form, fourth_form);
    let tokens = first_form.leaf_tokens();
    assert_eq!(second_form.leaf_tokens(), tokens);
    let doctest_groups: Vec<_> = base
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Doctest)
        .collect();
    assert_eq!(doctest_groups[0].tokens, tokens);
    let sites = [
        site("first", &blocks[0], &[]),
        site("second", &blocks[1], &[]),
    ];
    let reversed = [
        site("second", &blocks[1], &[]),
        site("first", &blocks[0], &[]),
    ];
    let included = group(&sites, 2, tokens);
    let reversed_report = group(&reversed, 2, tokens);
    assert_eq!(members(&included), [vec!["first", "second"]]);
    assert_eq!(members(&reversed_report), members(&included));
    assert_eq!(included.groups[0].id, reversed_report.groups[0].id);
    assert_eq!(included.groups[0].tokens, tokens);
    assert_eq!(group(&sites, 2, tokens + 1).groups.len(), 0);
    let changed = group(
        &[
            site("changed", &blocks[2], &[]),
            site("first", &blocks[0], &[]),
        ],
        2,
        tokens,
    );
    assert_eq!(changed.groups.len(), 0);
    let flagged = group(
        &[
            site("first", &blocks[0], &[]),
            site("second", &blocks[1], &[]),
            site("flag_first", &blocks[0], &["should_panic"]),
            site("flag_second", &blocks[1], &["should_panic"]),
            site("failing_first", &blocks[0], &["compile_fail"]),
            site("failing_second", &blocks[1], &["compile_fail"]),
        ],
        2,
        tokens,
    );
    assert_eq!(
        members(&flagged),
        [vec!["first", "second"], vec!["flag_first", "flag_second"],]
    );
}

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

fn distinct(left: &str, right: &str) {
    assert_ne!(form(left), form(right), "{left}\n{right}");
}

#[test]
fn structure_canonical_folds_keep_labels_and_effects() {
    same(
        "struct S{x:u32,y:u32}\nfn f(input:u32)->(u32,u32){let s=S{x:input&15u32,y:input>>4};(s.x,s.y)}",
        "struct S{x:u32,y:u32}\nfn f(input:u32)->(u32,u32){let s=S{y:input>>4,x:input&15u32};(s.x,s.y)}",
    );
    distinct(
        "struct S{x:u32,y:u32}\nfn f(input:u32)->(u32,u32){let s=S{x:input&15u32,y:input>>4};(s.x,s.y)}",
        "struct S{x:u32,y:u32}\nfn f(input:u32)->(u32,u32){let s=S{x:input>>4,y:input&15u32};(s.x,s.y)}",
    );
    same(
        "struct S{x:u32,y:u32}\nfn base(input:u32)->S{S{x:input,y:input}}\nfn f(input:u32)->(u32,u32){let s=S{x:input&15u32,y:input>>4,..base(input)};(s.x,s.y)}",
        "struct S{x:u32,y:u32}\nfn base(input:u32)->S{S{x:input,y:input}}\nfn f(input:u32)->(u32,u32){let s=S{y:input>>4,x:input&15u32,..base(input)};(s.x,s.y)}",
    );
    distinct(
        "struct S(u32,u32);\nfn f(input:u32)->(u32,u32){let s=S(input&15u32,input>>4);(s.0,s.1)}",
        "struct S(u32,u32);\nfn f(input:u32)->(u32,u32){let s=S(input>>4,input&15u32);(s.0,s.1)}",
    );
    let cases: Vec<SourcePair> =
        serde_json::from_str(include_str!("fixtures/dependency/structure_cases.json")).unwrap();
    let shadow = cases
        .iter()
        .find(|case| case.id == "S10_alias_shadowed_fields")
        .unwrap();
    distinct(&shadow.a, &shadow.b);
    assert_ne!(failing(&shadow.a), failing(&shadow.b));
    same(
        "fn note(input:u32)->u32{input}\nstruct S{x:u32,y:u32}\nfn f(input:u32)->(u32,u32){let s=S{x:input&15u32,y:note(input>>4)};(s.x,s.y)}",
        "fn note(input:u32)->u32{input}\nstruct S{x:u32,y:u32}\nfn f(input:u32)->(u32,u32){let s=S{y:note(input>>4),x:input&15u32};(s.x,s.y)}",
    );
    distinct(
        "fn fail(input:u32)->u32{if input>10u32{panic!(\"no\")}input}\nstruct S{x:u32,y:u32}\nfn f(input:u32)->(u32,u32){let s=S{x:input&15u32,y:fail(input>>4)};(s.x,s.y)}",
        "fn fail(input:u32)->u32{if input>10u32{panic!(\"no\")}input}\nstruct S{x:u32,y:u32}\nfn f(input:u32)->(u32,u32){let s=S{y:fail(input>>4),x:input&15u32};(s.x,s.y)}",
    );
    distinct(
        "fn fx(input:u32)->u32{println!(\"fx\");input&7u32}\nstruct S{x:u32,y:u32}\nfn f(input:u32)->u32{let s=S{x:fx(input),y:input>>4};s.x}",
        "fn fx(input:u32)->u32{println!(\"fx\");input&7u32}\nstruct S{x:u32,y:u32}\nfn f(input:u32)->u32{let s=S{y:input>>4,x:fx(input)};s.x}",
    );
}

#[test]
fn structure_canonical_alternatives_keep_payload_and_order() {
    same(
        "fn f(input:u32)->bool{match input{1|2|3=>true,_=>false}}",
        "fn f(input:u32)->bool{match input{3|1|2=>true,_=>false}}",
    );
    same(
        "fn f(input:u32)->bool{match input{2u32|1u32=>true,_=>false}}",
        "fn f(input:u32)->bool{match input{1u32|2u32=>true,_=>false}}",
    );
    distinct(
        "fn f(input:u32)->bool{match input{1|1|2=>true,_=>false}}",
        "fn f(input:u32)->bool{match input{1|2=>true,_=>false}}",
    );
    distinct(
        "fn f(input:u32)->u32{match input{2=>10u32,1=>20u32,_=>0u32}}",
        "fn f(input:u32)->u32{match input{1=>20u32,2=>10u32,_=>0u32}}",
    );
    distinct(
        "fn f(input:u32)->bool{match input{1|2 if input!=0=>true,_=>false}}",
        "fn f(input:u32)->bool{match input{2|1 if input!=0=>true,_=>false}}",
    );
    distinct(
        "fn f(input:u32)->u32{match input{n@1|n@2=>n,_=>0u32}}",
        "fn f(input:u32)->u32{match input{n@2|n@1=>n,_=>0u32}}",
    );
    distinct(
        "fn f(input:u32)->bool{match input{1|2..=2=>true,_=>false}}",
        "fn f(input:u32)->bool{match input{2..=2|1=>true,_=>false}}",
    );
    distinct(
        "macro_rules!two{()=>{2u32}}\nfn f(input:u32)->bool{match input{two!()|1=>true,_=>false}}",
        "macro_rules!two{()=>{2u32}}\nfn f(input:u32)->bool{match input{1|two!()=>true,_=>false}}",
    );
}

#[test]
fn structure_native_domains_and_observed_bases() {
    let cases: Vec<SourcePair> =
        serde_json::from_str(include_str!("fixtures/dependency/structure_cases.json")).unwrap();
    for id in [
        "S19_effectful_update_base",
        "S20_panic_update_base",
        "S21_bool_alternatives",
        "S22_char_alternatives",
        "S23_byte_alternatives",
        "S24_string_alternatives",
        "S25_u8_fields",
        "S26_u16_fields",
        "S27_u32_fields",
        "S28_u64_fields",
        "S29_u128_fields",
        "S30_i8_fields",
        "S31_i16_fields",
        "S32_i32_fields",
        "S33_i64_fields",
        "S34_i128_fields",
    ] {
        let case = cases.iter().find(|case| case.id == id).unwrap();
        same(&case.a, &case.b);
    }
    let observer = cases
        .iter()
        .find(|case| case.id == "S35_literal_payload_observer")
        .unwrap();
    distinct(&observer.a, &observer.b);
    assert_ne!(failing(&observer.a), failing(&observer.b));
}
