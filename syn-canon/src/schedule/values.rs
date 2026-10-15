//! Complete-scope primitive value erasure before exact scheduling.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use syn::visit::{self, Visit};

use super::{BlockScan, Lbl, MAX_BLOCK_DEPTH, MAX_REGION_NODES, Port, Renamer, Value};
use crate::scope::unraw;

struct Bounds {
    nodes: usize,
    expressions: usize,
    blocks: usize,
    outer: usize,
    valid: bool,
    observed: bool,
}

impl<'ast> Visit<'ast> for Bounds {
    fn visit_attribute(&mut self, _: &'ast syn::Attribute) {
        self.observed = true;
    }

    fn visit_block(&mut self, block: &'ast syn::Block) {
        let expressions = core::mem::replace(&mut self.expressions, 0);
        self.blocks += 1;
        if self.outer + self.blocks > MAX_BLOCK_DEPTH {
            self.valid = false;
        } else if self.valid {
            visit::visit_block(self, block);
        }
        self.blocks -= 1;
        self.expressions = expressions;
    }

    fn visit_stmt(&mut self, stmt: &'ast syn::Stmt) {
        self.nodes += 1;
        if self.nodes > MAX_REGION_NODES {
            self.valid = false;
        } else if self.valid {
            visit::visit_stmt(self, stmt);
        }
    }

    fn visit_item(&mut self, item: &'ast syn::Item) {
        self.nodes += 1;
        if self.nodes > MAX_REGION_NODES || matches!(item, syn::Item::Verbatim(_)) {
            self.valid = false;
        } else if self.valid {
            visit::visit_item(self, item);
        }
    }

    fn visit_expr(&mut self, expr: &'ast syn::Expr) {
        self.nodes += 1;
        self.expressions += 1;
        if self.nodes > MAX_REGION_NODES
            || self.expressions > MAX_BLOCK_DEPTH
            || matches!(expr, syn::Expr::Verbatim(_))
        {
            self.valid = false;
        } else if self.valid {
            visit::visit_expr(self, expr);
        }
        self.expressions -= 1;
    }
}

fn bounds(block: &syn::Block, outer: usize) -> Bounds {
    let mut scan = Bounds {
        nodes: 0,
        expressions: 0,
        blocks: 0,
        outer,
        valid: true,
        observed: false,
    };
    scan.visit_block(block);
    scan
}

pub(super) fn within_bounds(block: &syn::Block, outer: usize) -> bool {
    bounds(block, outer).valid
}

fn scalar_shape(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Lit(_) => true,
        syn::Expr::Path(path) => {
            path.qself.is_none()
                && path.path.leading_colon.is_none()
                && path.path.segments.len() == 1
                && matches!(path.path.segments[0].arguments, syn::PathArguments::None)
        }
        syn::Expr::Unary(unary) => scalar_shape(&unary.expr),
        syn::Expr::Binary(binary) => scalar_shape(&binary.left) && scalar_shape(&binary.right),
        syn::Expr::Group(group) => scalar_shape(&group.expr),
        syn::Expr::If(branch) => {
            scalar_shape(&branch.cond)
                && super::scalar_tail(&branch.then_branch).is_some_and(scalar_shape)
                && branch.else_branch.as_ref().is_some_and(|(_, otherwise)| {
                    matches!(&**otherwise, syn::Expr::Block(block)
                        if block.attrs.is_empty()
                            && block.label.is_none()
                            && super::scalar_tail(&block.block).is_some_and(scalar_shape))
                })
        }
        _ => false,
    }
}

fn value_shape(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Tuple(tuple) => tuple.elems.iter().all(value_shape),
        _ => scalar_shape(expr),
    }
}

fn plain_binding(pat: &syn::Pat) -> Option<String> {
    match pat {
        syn::Pat::Ident(ident)
            if ident.by_ref.is_none() && ident.mutability.is_none() && ident.subpat.is_none() =>
        {
            Some(unraw(&ident.ident.to_string()).to_string())
        }
        _ => None,
    }
}

struct Definition<'ast> {
    stmt: usize,
    expr: &'ast syn::Expr,
    node: usize,
    sources: Vec<super::Site>,
}

fn definitions<'ast>(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    local: &'ast syn::Local,
) -> Vec<(String, &'ast syn::Expr, Lbl, Vec<super::Site>)> {
    let Some(init) = &local.init else {
        return Vec::new();
    };
    if init.diverge.is_some() {
        return Vec::new();
    }
    let pairs: Vec<_> = match (&local.pat, &*init.expr) {
        (pat @ syn::Pat::Ident(_), expr) => plain_binding(pat)
            .map(|name| vec![(name, expr)])
            .unwrap_or_default(),
        (syn::Pat::Tuple(pattern), syn::Expr::Tuple(tuple))
            if !pattern.elems.is_empty() && pattern.elems.len() == tuple.elems.len() =>
        {
            let Some(names) = pattern
                .elems
                .iter()
                .map(plain_binding)
                .collect::<Option<Vec<_>>>()
            else {
                return Vec::new();
            };
            names.into_iter().zip(&tuple.elems).collect()
        }
        _ => return Vec::new(),
    };
    let mut result = Vec::with_capacity(pairs.len());
    for (name, expr) in pairs {
        if !scalar_shape(expr) {
            return Vec::new();
        }
        let mut locals = 0;
        let (label, sites) = super::label_expr(renamer, env, expr, 0, &mut locals, None);
        if label.prim().is_none() {
            return Vec::new();
        }
        result.push((name, expr, label, sites));
    }
    result
}

struct Expanded {
    expr: syn::Expr,
    references: BTreeMap<String, Value>,
    nodes: usize,
}

fn expression_bounds(expr: &syn::Expr, outer: usize) -> Bounds {
    let mut scan = Bounds {
        nodes: 0,
        expressions: 0,
        blocks: 0,
        outer,
        valid: true,
        observed: false,
    };
    scan.visit_expr(expr);
    scan
}

fn expand(
    expr: &mut syn::Expr,
    syntax: &mut crate::helpers::HelperSyntax,
    original: &BTreeMap<String, Value>,
    retained: &BTreeMap<String, Value>,
    replacements: &BTreeMap<Port, Expanded>,
    nodes: &mut usize,
) -> bool {
    struct Expansion<'a> {
        syntax: &'a mut crate::helpers::HelperSyntax,
        original: &'a BTreeMap<String, Value>,
        retained: &'a BTreeMap<String, Value>,
        replacements: &'a BTreeMap<Port, Expanded>,
        nodes: &'a mut usize,
        valid: bool,
    }
    impl syn::visit_mut::VisitMut for Expansion<'_> {
        fn visit_expr_mut(&mut self, expr: &mut syn::Expr) {
            if !self.valid {
                return;
            }
            if let syn::Expr::Path(path) = expr {
                let name = path.path.segments[0].ident.to_string();
                let value = self
                    .original
                    .get(unraw(&name))
                    .expect("an admitted primitive path");
                if let Some(replacement) = self.replacements.get(&value.port) {
                    if replacement
                        .references
                        .iter()
                        .any(|(name, value)| self.retained.get(name) != Some(value))
                    {
                        self.valid = false;
                        return;
                    }
                    let expanded = *self.nodes + replacement.nodes - 1;
                    if expanded > MAX_REGION_NODES {
                        self.valid = false;
                        return;
                    }
                    *self.nodes = expanded;
                    self.syntax.clear_expr(expr);
                    *expr = self.syntax.clone_expr(&replacement.expr);
                } else {
                    self.valid = self.retained.get(unraw(&name)) == Some(value);
                }
            } else {
                syn::visit_mut::visit_expr_mut(self, expr);
            }
        }
    }
    let mut expansion = Expansion {
        syntax,
        original,
        retained,
        replacements,
        nodes,
        valid: true,
    };
    syn::visit_mut::VisitMut::visit_expr_mut(&mut expansion, expr);
    expansion.valid
}

fn references(expr: &syn::Expr, env: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    struct References<'a> {
        env: &'a BTreeMap<String, Value>,
        values: BTreeMap<String, Value>,
    }
    impl<'ast> Visit<'ast> for References<'_> {
        fn visit_expr_path(&mut self, path: &'ast syn::ExprPath) {
            let mut name = path.path.segments[0].ident.to_string();
            if name.starts_with("r#") {
                name.drain(..2);
            }
            let value = self
                .env
                .get(&name)
                .expect("an expanded primitive path retains its source");
            self.values.entry(name).or_insert_with(|| value.clone());
        }
    }
    let mut references = References {
        env,
        values: BTreeMap::new(),
    };
    references.visit_expr(expr);
    references.values
}

pub(super) enum Outcome {
    Unchanged,
    Rewritten(syn::Block),
    Exhausted,
}

#[expect(
    clippy::too_many_lines,
    reason = "Complete-scope proof and atomic rewriting share original binding identities"
)]
pub(super) fn normalize(renamer: &mut Renamer, block: &mut syn::Block) -> Outcome {
    if !renamer.values_allowed()
        || !block
            .stmts
            .iter()
            .any(|stmt| matches!(stmt, syn::Stmt::Local(_)))
    {
        return Outcome::Unchanged;
    }
    let original_bounds = bounds(block, renamer.depth() - 1);
    if !original_bounds.valid {
        return Outcome::Exhausted;
    }
    if original_bounds.observed {
        return Outcome::Unchanged;
    }
    let mut env = BTreeMap::new();
    renamer.frame_env(&mut env);
    let locals = renamer.scopes.last().expect("a function scope").next_local;
    let mut scan = BlockScan::new(env.clone(), locals, block.stmts.len());
    let mut defs = Vec::new();
    let mut changes = Vec::with_capacity(block.stmts.len());
    let mut approved = Vec::with_capacity(block.stmts.len());
    for (index, stmt) in block.stmts.iter().enumerate() {
        let mut admitted = false;
        if !scan.unreachable
            && let syn::Stmt::Local(local) = stmt
        {
            let candidates = definitions(renamer, &scan.env, local);
            admitted = !candidates.is_empty();
            if admitted {
                for (name, expr, label, sites) in candidates {
                    scan.budget(&label);
                    let node = scan.producers.len();
                    scan.producer(index, name, label, &sites);
                    defs.push(Definition {
                        stmt: index,
                        expr,
                        node,
                        sources: sites,
                    });
                }
            } else {
                scan.other(renamer, index, stmt);
                for name in super::pat_names(&local.pat) {
                    scan.invalidate(name);
                }
            }
        } else {
            if let syn::Stmt::Expr(expr, None) = stmt {
                admitted = !scan.unreachable
                    && value_shape(expr)
                    && super::tail_root(renamer, &scan.env, expr, None);
            }
            if !admitted {
                scan.statement(renamer, index, stmt, false);
            }
        }
        let names = match stmt {
            syn::Stmt::Local(local) => super::pat_names(&local.pat),
            _ => super::assign_target(stmt).into_iter().collect(),
        };
        changes.push(
            names
                .into_iter()
                .filter_map(|name| scan.env.get(&name).map(|value| (name, value.clone())))
                .collect::<Vec<_>>(),
        );
        approved.push(admitted);
    }
    if scan.over_budget {
        return Outcome::Exhausted;
    }
    let scope = renamer.scopes.last_mut().expect("a function scope");
    scope.observed.extend(
        scan.observed
            .iter()
            .filter(|port| !matches!(port, Port::Node(_)))
            .cloned(),
    );
    let mut erase: Vec<_> = block
        .stmts
        .iter()
        .enumerate()
        .map(|(index, stmt)| approved[index] && matches!(stmt, syn::Stmt::Local(_)))
        .collect();
    for def in &defs {
        let id = def.node;
        let allowed = !scan.regions[scan.producers[id].region].dead
            && !scan.forbidden.contains(&Port::Node(id))
            && !def.sources.iter().any(|site| {
                scan.forbidden.contains(&site.port) || scope.observed.contains(&site.port)
            });
        erase[def.stmt] &= allowed;
    }
    if !erase.iter().any(|&erased| erased) {
        return Outcome::Unchanged;
    }
    let mut syntax = crate::helpers::HelperSyntax::default();
    let mut rewritten = renamer.helper_syntax.clone_block_into(block, &mut syntax);
    let mut retained = env.clone();
    let mut replacements = BTreeMap::new();
    let mut nodes = original_bounds.nodes;
    let mut definition_index = 0;
    let mut cached_nodes = 0;
    for (index, stmt) in rewritten.stmts.iter_mut().enumerate() {
        let first_definition = definition_index;
        while defs
            .get(definition_index)
            .is_some_and(|def| def.stmt == index)
        {
            definition_index += 1;
        }
        if erase[index] {
            let original = &block.stmts[index];
            let mut removed = Bounds {
                nodes: 0,
                expressions: 0,
                blocks: 0,
                outer: 0,
                valid: true,
                observed: false,
            };
            removed.visit_stmt(original);
            nodes -= removed.nodes;
            for def in &defs[first_definition..definition_index] {
                let mut expression = renamer.helper_syntax.clone_expr_into(def.expr, &mut syntax);
                let mut expansion_nodes = nodes;
                if !expand(
                    &mut expression,
                    &mut syntax,
                    &env,
                    &retained,
                    &replacements,
                    &mut expansion_nodes,
                ) {
                    return Outcome::Exhausted;
                }
                let checked = expression_bounds(&expression, renamer.depth());
                cached_nodes += checked.nodes;
                if !checked.valid || cached_nodes > MAX_REGION_NODES {
                    return Outcome::Exhausted;
                }
                replacements.insert(
                    Port::Node(def.node),
                    Expanded {
                        references: references(&expression, &retained),
                        nodes: checked.nodes,
                        expr: expression,
                    },
                );
            }
        } else {
            let expr = match stmt {
                syn::Stmt::Local(local) if approved[index] => {
                    Some(&mut *local.init.as_mut().expect("an admitted initializer").expr)
                }
                syn::Stmt::Expr(expr, _) if approved[index] => Some(expr),
                _ => None,
            };
            if let Some(expr) = expr
                && !expand(
                    expr,
                    &mut syntax,
                    &env,
                    &retained,
                    &replacements,
                    &mut nodes,
                )
            {
                return Outcome::Exhausted;
            }
        }
        for (name, value) in &changes[index] {
            env.insert(name.clone(), value.clone());
            if !erase[index] {
                retained.insert(name.clone(), value.clone());
            }
        }
    }
    let mut index = 0;
    rewritten.stmts.retain(|_| {
        let keep = !erase[index];
        index += 1;
        keep
    });
    if !bounds(&rewritten, renamer.depth() - 1).valid {
        return Outcome::Exhausted;
    }
    syntax.retain_block(&rewritten);
    renamer.helper_syntax.extend(syntax);
    Outcome::Rewritten(core::mem::replace(block, rewritten))
}
