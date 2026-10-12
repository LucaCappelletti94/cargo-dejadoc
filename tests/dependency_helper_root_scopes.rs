//! Original transitive helper budgets follow lexical binding lifetimes.

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

fn view(source: &str) -> syn_canon::CanonicalForm {
    let file: syn::File = syn::parse_str(source).unwrap();
    let context = syn_canon::SourceContext::new(core::iter::once((&[][..], &file)));
    let function = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == "f" => Some(function),
            _ => None,
        })
        .unwrap();
    context
        .function(&function.sig, &function.block)
        .unwrap()
        .canonicalize()
}

fn pair(first: &str, second: &str, equal: bool) {
    for (api, canonicalize) in [
        ("standalone", form as fn(&str) -> syn_canon::CanonicalForm),
        ("borrowed", view),
    ] {
        assert_eq!(
            canonicalize(first) == canonicalize(second),
            equal,
            "{api} preserves the original helper union and binding scopes"
        );
    }
}

fn source(prefix: &str, statements: &str, repetitions: usize) -> String {
    format!(
        "fn f(p:bool,callback:fn(bool)->bool)->(bool,bool){{{prefix}{statements}(a,b)}}fn h(p:bool)->bool{{g(p)}}fn g(p:bool)->bool{{{}p}}fn k(p:bool)->bool{{false;false;false;false;p}}",
        "p;".repeat(repetitions)
    )
}

fn union_pair(prefix: &str, first: &str, second: &str) {
    pair(
        &source(prefix, first, 8187),
        &source(prefix, second, 8187),
        false,
    );
    pair(
        &source(prefix, first, 8182),
        &source(prefix, second, 8182),
        true,
    );
}

#[derive(Default)]
struct DeclarationNodes(usize);

impl syn::visit::Visit<'_> for DeclarationNodes {
    fn visit_stmt(&mut self, stmt: &syn::Stmt) {
        self.0 += 1;
        syn::visit::visit_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &syn::Expr) {
        self.0 += 1;
        syn::visit::visit_expr(self, expr);
    }

    fn visit_item(&mut self, item: &syn::Item) {
        self.0 += 1;
        syn::visit::visit_item(self, item);
    }
}

fn declaration_nodes(source: &str, names: &[&str]) -> usize {
    let file: syn::File = syn::parse_str(source).unwrap();
    let mut count = DeclarationNodes::default();
    for item in &file.items {
        if let syn::Item::Fn(function) = item
            && names.iter().any(|name| function.sig.ident == *name)
        {
            count.0 += 1;
            syn::visit::visit_block(&mut count, &function.block);
        }
    }
    count.0
}

#[test]
fn only_the_complete_transitive_union_exceeds_the_node_cap() {
    let exhausted = source("", "let a=h(p);let b=k(!p);", 8187);
    let admitted = source("", "let a=h(p);let b=k(!p);", 8182);
    assert_eq!(declaration_nodes(&exhausted, &["h", "g"]), 16_382);
    assert_eq!(declaration_nodes(&exhausted, &["h", "g", "k"]), 16_393);
    assert_eq!(declaration_nodes(&admitted, &["h", "g", "k"]), 16_383);
    pair(
        &exhausted,
        &source("", "let b=k(!p);let a=h(p);", 8187),
        false,
    );
    pair(
        &admitted,
        &source("", "let b=k(!p);let a=h(p);", 8182),
        true,
    );
}

#[test]
fn completed_patterns_restore_later_enclosing_helper_roots() {
    for prefix in [
        "match Some(p){Some(h)=>{let _=h;},None=>{}}",
        "for h in [p]{let _=h;}",
        "if let Some(h)=Some(p){let _=h;}",
        "while let Some(h)=Some(p){let _=h;break;}",
        "if let Some(h)=Some(p)&&h{let _=h;}",
        "while let Some(h)=Some(p)&&h{let _=h;break;}",
        "{let h=p;let _=h;}",
        "let closure=|h:bool|h;let _=closure(p);",
    ] {
        union_pair(prefix, "let a=h(p);let b=k(!p);", "let b=k(!p);let a=h(p);");
    }
}

#[test]
fn match_arms_restore_roots_for_the_next_arm_and_discriminant() {
    for prefix in [
        "match Some(p){Some(h)=>{let _=h;},None=>{let _=h(p);}}",
        "match Some(h(p)){Some(h)=>{let _=h;},None=>{}}",
        "match Some(p){Some(q)if h(q)=>{},_=>{}}",
    ] {
        union_pair(prefix, "let a=k(p);let b=k(!p);", "let b=k(!p);let a=k(p);");
    }
}

#[test]
fn if_condition_bindings_do_not_mask_else_roots() {
    for prefix in [
        "if let Some(h)=Some(p){let _=h;}else{let _=h(p);}",
        "if let Some(h)=Some(p)&&h{let _=h;}else{let _=h(p);}",
        "if let Some(h)=Some(p){let _=h;}else if let Some(h)=Some(h(p)){let _=h;}",
    ] {
        union_pair(prefix, "let a=k(p);let b=k(!p);", "let b=k(!p);let a=k(p);");
    }
}

#[test]
fn iterators_and_let_initializers_resolve_before_their_patterns() {
    for prefix in [
        "for h in [h(p)]{let _=h;}",
        "if let Some(h)=Some(h(p)){let _=h;}",
        "while let Some(h)=Some(h(p)){let _=h;break;}",
        "if let Some(q)=Some(p)&&let Some(h)=Some(h(q)){let _=h;}",
        "while let Some(q)=Some(p)&&let Some(h)=Some(h(q)){let _=h;break;}",
        "let h=h(p);let _=h;",
        "let Some(h)=Some(h(p))else{return(p,p)};let _=h;",
    ] {
        union_pair(prefix, "let a=k(p);let b=k(!p);", "let b=k(!p);let a=k(p);");
    }
}

#[test]
fn live_callable_patterns_mask_farther_roots_without_exhaustion() {
    for prefix in [
        "match Some(callback){Some(h)if h(p)=>{let _=h(!p);},Some(_)=>{},None=>{}}",
        "for h in [callback]{let _=h(p);}",
        "if let Some(h)=Some(callback){let _=h(p);}",
        "while let Some(h)=Some(callback){let _=h(p);break;}",
        "if let Some(h)=Some(callback)&&h(p)&&let Some(q)=Some(h(!p)){let _=h(q);}",
        "while let Some(h)=Some(callback)&&h(p)&&let Some(q)=Some(h(!p)){let _=h(q);break;}",
        "{let h=callback;let _=h(p);}",
        "let closure=|h:fn(bool)->bool|h(p);let _=closure(callback);",
        "let h=callback;{let _=h(p);}let closure=||h(!p);let _=closure();",
    ] {
        pair(
            &source(prefix, "let a=k(p);let b=k(!p);", 8187),
            &source(prefix, "let b=k(!p);let a=k(p);", 8187),
            true,
        );
    }
}

#[test]
fn nested_blocks_and_closures_still_include_unmasked_roots() {
    for prefix in [
        "{let _=h(p);}",
        "let closure=||h(p);let _=closure();",
        "let closure=|q:bool|h(q);let _=closure(p);",
        "if let Some(h)=Some(callback){let closure=|q:bool|h(q);let _=closure(p);}let _=h(p);",
    ] {
        union_pair(prefix, "let a=k(p);let b=k(!p);", "let b=k(!p);let a=k(p);");
    }
}

#[test]
fn exhausted_scope_unions_keep_dead_enclosing_bindings() {
    for prefix in [
        "match Some(p){Some(h)=>{let _=h;},None=>{}}",
        "for h in [p]{let _=h;}",
        "if let Some(h)=Some(p){let _=h;}",
        "while let Some(h)=Some(p){let _=h;break;}",
        "if let Some(h)=Some(p)&&h{let _=h;}",
        "while let Some(h)=Some(p)&&h{let _=h;break;}",
    ] {
        let plain = source(prefix, "let a=h(p);let b=k(!p);", 8187);
        let dead = source(prefix, "let unused=false;let a=h(p);let b=k(!p);", 8187);
        pair(&plain, &dead, false);
    }
}
