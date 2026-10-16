//! Source-AST budget endpoints of the complete-scope value pass.

fn tree(leaves: &[&str]) -> String {
    if leaves.len() == 1 {
        return leaves[0].to_owned();
    }
    let mid = leaves.len() / 2;
    format!("({}^{})", tree(&leaves[..mid]), tree(&leaves[mid..]))
}

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

// The balanced 8190-leaf tail has 16379 expressions after parenthesis folding.
fn node_tail(leaf: &str) -> String {
    let mut leaves = vec![leaf];
    leaves.resize(8_190, "x");
    tree(&leaves)
}

// The declaration and tail total 16384 nodes; a zero-initializer local adds one.
fn node_region(zeros: usize, leaf: &str) -> String {
    let zeros: Vec<String> = (0..zeros)
        .map(|index| format!("let _z{index}: u8;"))
        .collect();
    let declaration = if leaf == "t" { "let t=x&1u8;" } else { "" };
    format!(
        "fn f(x:u8)->u8{{{declaration}{}{}}}",
        zeros.join(""),
        node_tail(leaf)
    )
}

#[test]
fn source_node_limit_admits_the_exact_complete_region() {
    let original = node_region(0, "t");
    let inline = node_region(0, "(x&1u8)");
    assert_eq!(form(&original), form(&inline));
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(&original).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(&inline).unwrap())
    );
}

#[test]
fn source_node_exhaustion_retains_the_complete_region() {
    let original = node_region(1, "t");
    let variant = format!(
        "fn f(x:u8)->u8{{let t=x&1_u8;let _z0:u8;{}}}",
        node_tail("t")
    );
    let inline = node_region(1, "(x&1u8)");
    let retained = form(&original);
    assert_eq!(retained, form(&variant));
    assert_ne!(retained, form(&inline));
    assert_eq!(retained.body_units(), 16_391);
}

// 62 nested block expressions reach the 64th level with the two function bodies; 63 cross to 65.
fn lexical_source(nests: usize, leaf: &str, erasing: bool) -> String {
    let inner = (0..nests).fold(leaf.to_string(), |value, _| format!("{{{value}}}"));
    let g = format!("fn g()->u8{{{inner}}}");
    if erasing {
        format!("fn f(x:u8)->u8{{{g};let t=x&1u8;t}}")
    } else {
        format!("fn f(x:u8)->u8{{{g};x&1u8}}")
    }
}

#[test]
fn source_block_limit_admits_the_exact_complete_region() {
    let original = lexical_source(62, "1u8", true);
    let inline = lexical_source(62, "1u8", false);
    assert_eq!(form(&original), form(&inline));
}

#[test]
fn source_block_exhaustion_retains_the_complete_region() {
    let original = lexical_source(63, "1u8", true);
    let variant = lexical_source(63, "1_u8", true);
    let inline = lexical_source(63, "1u8", false);
    let retained = form(&original);
    assert_eq!(retained, form(&variant));
    assert_ne!(retained, form(&inline));
}

#[test]
fn let_else_declaration_survives_value_erasure() {
    let original = "fn f(x:u8)->u8{let t=x&1u8;let e=0u8 else{loop{}};t}";
    let retained = "fn f(x:u8)->u8{let e=0u8 else{loop{}};x&1u8}";
    assert_eq!(form(original), form(retained));
}

#[test]
fn block_attribute_retains_the_candidate_declaration() {
    for (attributed, variant) in [
        (
            "fn f(x:u8)->u8{#[must_use] let t=x&1u8;t}",
            "fn f(x:u8)->u8{#[must_use] let t=x&1_u8;t}",
        ),
        (
            "fn f(x:u8)->u8{let t=#[inspect](x&1u8);t}",
            "fn f(x:u8)->u8{let t=#[inspect](x&1_u8);t}",
        ),
    ] {
        let retained = form(attributed);
        assert_eq!(retained, form(variant));
        assert_ne!(retained, form("fn f(x:u8)->u8{x&1u8}"));
    }
}

#[test]
fn nonliteral_format_retains_the_declaration_at_the_opaque_boundary() {
    let copied = "fn f(x:u8){let copied=x;println!(concat!(\"{}\", \"!\"),copied);}";
    let renamed = "fn f(y:u8){let copied=y;println!(concat!(\"{}\", \"!\"),copied);}";
    let direct = "fn f(x:u8){println!(concat!(\"{}\", \"!\"),x);}";
    let retained = form(copied);
    assert_eq!(retained, form(renamed));
    assert_ne!(retained, form(direct));
}

fn item_endpoint(zeros: usize, literal: &str) -> String {
    use std::fmt::Write as _;
    let mut source = format!("fn f(){{let marker={literal};");
    for _ in 0..8_189 {
        source.push_str("0u32;");
    }
    for index in 0..zeros {
        write!(source, "let spare{index}:u32;").unwrap();
    }
    source.push_str("struct End;}");
    source
}

#[test]
fn source_item_limit_admits_the_exact_complete_region() {
    let left = item_endpoint(2, "7u32");
    let right = item_endpoint(2, "9u32");
    assert_eq!(form(&left), form(&right));
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(&left).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(&right).unwrap())
    );
}

#[test]
fn source_item_exhaustion_retains_the_complete_region() {
    assert_ne!(
        form(&item_endpoint(3, "7u32")),
        form(&item_endpoint(3, "9u32"))
    );
}

fn item_prefix(structs: usize, literal: &str) -> String {
    use std::fmt::Write as _;
    let mut source = String::from("fn f(){const ZERO:u32=0u32;");
    for index in 0..structs {
        write!(source, "struct End{index};").unwrap();
    }
    write!(source, "let marker={literal};}}").unwrap();
    source
}

#[test]
fn exhausted_item_prefix_retains_the_dead_binding() {
    assert_eq!(
        form(&item_prefix(8_189, "7u32")),
        form(&item_prefix(8_189, "9u32"))
    );
    assert_ne!(
        form(&item_prefix(8_191, "7u32")),
        form(&item_prefix(8_191, "9u32"))
    );
}

#[test]
fn exhausted_nested_signature_retains_the_complete_enclosing_block() {
    let length = vec!["SIZE"; 65].join("&");
    let original =
        format!("const SIZE:usize=1;fn f(){{fn g(_: [u8;{length}]){{}}let marker=7u32;}}");
    let variant = original.replace("7u32", "7_u32");
    let inline = format!("const SIZE:usize=1;fn f(){{fn g(_: [u8;{length}]){{}}}}");
    assert_eq!(form(&original), form(&variant));
    assert_ne!(form(&original), form(&inline));
}

fn grouped_format(literal: &str) -> syn_canon::CanonicalForm {
    let source = format!("fn f(){{let marker={literal};println!(\"done\");}}");
    let mut file: syn::File = syn::parse_str(&source).unwrap();
    let syn::Item::Fn(function) = &mut file.items[0] else {
        unreachable!()
    };
    let syn::Stmt::Macro(statement) = &mut function.block.stmts[1] else {
        unreachable!()
    };
    statement.mac.tokens = proc_macro2::TokenTree::Group(proc_macro2::Group::new(
        proc_macro2::Delimiter::None,
        "\"done\"".parse().unwrap(),
    ))
    .into();
    syn_canon::canonicalize(file)
}

#[test]
fn generated_format_operand_retains_every_visible_value_source() {
    assert_ne!(grouped_format("7u32"), grouped_format("9u32"));
    assert_eq!(grouped_format("7u32"), grouped_format("7_u32"));
    assert_eq!(
        form("fn f(){let marker=7u32;println!(\"done\");}"),
        form("fn f(){let marker=9u32;println!(\"done\");}")
    );
}

#[test]
fn exhausted_module_initializer_retains_later_items_and_enclosing_values() {
    let value = vec!["super::VALUE"; 65].join("&");
    let original = format!(
        "const VALUE:u8=1;fn f(){{mod inner{{const LONG:u8={value};struct End;}}let marker=7u32;}}"
    );
    let variant = original.replace("7u32", "7_u32");
    let inline =
        format!("const VALUE:u8=1;fn f(){{mod inner{{const LONG:u8={value};struct End;}}}}");
    assert_eq!(form(&original), form(&variant));
    assert_ne!(form(&original), form(&inline));
}
