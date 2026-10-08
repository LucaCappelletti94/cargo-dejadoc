//! Conservative proof metadata of original function blocks.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use syn::visit::Visit;

use crate::alpha::{MacroArgs, Renamer};
use crate::reference::BlockProof;
use crate::scope::{PrimTy, unraw};

/// The block nesting level a function scope may analyze.
const MAX_BLOCK_DEPTH: usize = 64;

/// A proven primitive declaration.
const PRODUCER: u8 = b'P';
/// A known observation with modeled value inputs.
const OBSERVATION: u8 = b'K';
/// A control boundary or a possible divergence.
const BOUNDARY: u8 = b'B';
/// A tail read that never runs.
const UNREACHABLE: u8 = b'U';
/// A proven tail read.
const TAIL: u8 = b'T';
/// Syntax without a motion proof.
const OPAQUE: u8 = b'O';

/// The std macros the analysis models as observations with value inputs.
const OBSERVATION_MACROS: [&str; 8] = [
    "eprint",
    "eprintln",
    "format",
    "format_args",
    "print",
    "println",
    "write",
    "writeln",
];
/// The std macros the analysis models as possible divergences.
const DIVERGING_MACROS: [&str; 10] = [
    "assert",
    "assert_eq",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "panic",
    "todo",
    "unimplemented",
    "unreachable",
];

/// The scalar fact an expression carries, the total type when proven.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lbl {
    /// An integer literal, `None` when the suffix leaves the type unproven.
    Int(u128, Option<PrimTy>),
    /// A boolean literal.
    Bool(bool),
    /// A resolved value read or a proven total operation.
    Value(Option<PrimTy>),
    /// Any other syntax, no total type.
    Other,
}

impl Lbl {
    /// The proven scalar type of a total expression, `None` otherwise.
    fn prim(&self) -> Option<PrimTy> {
        match self {
            Self::Int(_, prim) | Self::Value(prim) => *prim,
            Self::Bool(_) => Some(PrimTy::Bool),
            Self::Other => None,
        }
    }
}

/// A value the environment holds, by original name.
#[derive(Clone, Copy)]
pub(crate) struct Value {
    /// The producer statement index the value comes from, when one is proven.
    pub(crate) producer: Option<usize>,
    /// The proven fixed-width primitive scalar type.
    pub(crate) prim: Option<PrimTy>,
}

/// One reference of a scanned expression to a held value.
#[derive(Clone, Copy)]
struct Site {
    producer: Option<usize>,
    storage: bool,
}

/// A proven producer of the block being analyzed.
struct Producer {
    /// The statement index in the block.
    stmt: usize,
    prim: PrimTy,
    dead: bool,
}
struct Scan {
    env: BTreeMap<String, Value>,
    producers: Vec<Producer>,
    roles: Vec<u8>,
    types: Vec<Option<PrimTy>>,
    tail_types: Vec<Option<PrimTy>>,
    unreachable: bool,
    tail_pending: bool,
}

impl Scan {
    fn new(env: BTreeMap<String, Value>, count: usize) -> Self {
        Self {
            env,
            producers: Vec::new(),
            roles: vec![OPAQUE; count],
            types: vec![None; count],
            tail_types: Vec::new(),
            unreachable: false,
            tail_pending: false,
        }
    }

    /// Mark the producers whose storage the sites observe.
    fn apply(&mut self, sites: &[Site]) {
        for site in sites {
            if site.storage
                && let Some(producer) = site.producer
            {
                self.producers[producer].dead = true;
            }
        }
    }

    fn statement(&mut self, renamer: &Renamer, index: usize, stmt: &syn::Stmt, last: bool) {
        if last
            && let syn::Stmt::Expr(expr, None) = stmt
            && (self.unreachable || tail_root(renamer, &self.env, expr))
        {
            self.tail_pending = true;
            return;
        }
        if self.unreachable {
            let (_, sites) = statement_label(renamer, &self.env, stmt);
            self.apply(&sites);
            return;
        }
        if let syn::Stmt::Local(local) = stmt {
            self.local(renamer, index, stmt, local);
        } else {
            self.other(renamer, index, stmt);
        }
    }

    fn local(&mut self, renamer: &Renamer, index: usize, stmt: &syn::Stmt, local: &syn::Local) {
        if let Some((name, init, sites)) = let_producer(renamer, &self.env, local)
            && init.prim().is_some()
        {
            self.producer(index, name, init, &sites);
            return;
        }
        let (_, sites) = statement_label(renamer, &self.env, stmt);
        self.apply(&sites);
        if let Some(name) = local_binding_name(local) {
            self.invalidate(name);
        }
    }

    fn producer(&mut self, index: usize, name: String, init: Lbl, sites: &[Site]) {
        let prim = init.prim().expect("a proven initializer");
        self.apply(sites);
        let id = self.producers.len();
        self.producers.push(Producer {
            stmt: index,
            prim,
            dead: false,
        });
        self.env.insert(
            name,
            Value {
                producer: Some(id),
                prim: Some(prim),
            },
        );
    }

    fn other(&mut self, renamer: &Renamer, index: usize, stmt: &syn::Stmt) {
        let kind = stmt_kind(renamer, stmt);
        let (_, sites) = statement_label(renamer, &self.env, stmt);
        match kind {
            StmtKind::Barrier => {
                self.roles[index] = OBSERVATION;
                self.apply(&sites);
            }
            StmtKind::Boundary(definite) => {
                self.roles[index] = BOUNDARY;
                self.apply(&sites);
                self.unreachable = definite;
                return;
            }
            StmtKind::Plain => self.apply(&sites),
        }
        if let Some(target) = assign_target(stmt) {
            if let Some(value) = self.env.get(&target)
                && let Some(producer) = value.producer
            {
                self.producers[producer].dead = true;
            }
            self.invalidate(target);
        }
    }

    fn invalidate(&mut self, name: String) {
        self.env.insert(
            name,
            Value {
                producer: None,
                prim: None,
            },
        );
    }

    fn finish_tail(&mut self, renamer: &Renamer, block: &syn::Block) {
        if !self.tail_pending {
            return;
        }
        let Some(syn::Stmt::Expr(expr, None)) = block.stmts.last() else {
            unreachable!("a pending tail is an expression");
        };
        let role = self
            .roles
            .last_mut()
            .expect("a pending tail has a statement");
        if self.unreachable {
            *role = UNREACHABLE;
            return;
        }
        *role = TAIL;
        if let syn::Expr::Tuple(tuple) = peel_paren(expr) {
            for element in &tuple.elems {
                self.tail_element(renamer, element);
            }
        } else {
            self.tail_element(renamer, expr);
        }
    }

    fn tail_element(&mut self, renamer: &Renamer, expr: &syn::Expr) {
        let (label, sites) = label_expr(renamer, &self.env, expr);
        self.apply(&sites);
        self.tail_types.push(label.prim());
    }
}

/// Resolve `block` under the innermost function scope, returning its
/// conservative proof and the proven types of its producer patterns.
pub(crate) fn analyze(
    renamer: &Renamer,
    block: &syn::Block,
) -> (BlockProof, BTreeMap<*const syn::Pat, PrimTy>) {
    if renamer.proof_frozen() || renamer.depth() > MAX_BLOCK_DEPTH {
        return (
            BlockProof {
                statements: vec![OPAQUE; block.stmts.len()],
                types: vec![None; block.stmts.len()],
                tail_types: Vec::new(),
            },
            BTreeMap::new(),
        );
    }
    let mut env = BTreeMap::new();
    renamer.proof_env(&mut env);
    let mut scan = Scan::new(env, block.stmts.len());
    for (index, stmt) in block.stmts.iter().enumerate() {
        scan.statement(renamer, index, stmt, index + 1 == block.stmts.len());
    }
    scan.finish_tail(renamer, block);
    let mut pats = BTreeMap::new();
    for producer in &scan.producers {
        if producer.dead {
            continue;
        }
        scan.roles[producer.stmt] = PRODUCER;
        scan.types[producer.stmt] = Some(producer.prim);
        let syn::Stmt::Local(local) = &block.stmts[producer.stmt] else {
            unreachable!("a producer is a local declaration");
        };
        pats.insert(core::ptr::from_ref(&local.pat), producer.prim);
    }
    (
        BlockProof {
            statements: scan.roles,
            types: scan.types,
            tail_types: scan.tail_types,
        },
        pats,
    )
}

/// Whether the function body of `block` reaches a macro call the analysis
/// does not model.
pub(crate) fn unknown_macro_reaches(renamer: &Renamer, block: &syn::Block) -> bool {
    let mut scan = UnknownScan {
        renamer,
        found: false,
    };
    syn::visit::visit_block(&mut scan, block);
    scan.found
}

struct UnknownScan<'a> {
    renamer: &'a Renamer<'a>,
    found: bool,
}

impl UnknownScan<'_> {
    /// Whether the macro call's origin cannot be proven: an unmodeled
    /// name, or a name the shared resolver shadows.
    fn uncertain(&self, mac: &syn::Macro) -> bool {
        if macro_kind(&mac.path).is_none() {
            return true;
        }
        let head = mac.path.segments[0].ident.to_string();
        if mac.path.segments.len() == 1 {
            self.renamer.macro_shadowed(unraw(&head))
        } else {
            self.renamer.macro_shadowed(unraw(&head)) || self.renamer.type_shadowed(unraw(&head))
        }
    }
}

impl<'ast> Visit<'ast> for UnknownScan<'_> {
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if self.uncertain(mac) {
            self.found = true;
            return;
        }
        for arg in macro_args(mac) {
            self.visit_expr(&arg);
        }
    }
}

/// The macro kind a call names: `None` unknown, `Some(true)` diverging,
/// `Some(false)` observation or stable.
fn macro_kind(path: &syn::Path) -> Option<bool> {
    let name = match path.segments.len() {
        1 if path.leading_colon.is_none() => path.segments[0].ident.to_string(),
        2 => {
            let head = path.segments[0].ident.to_string();
            if !matches!(unraw(&head), "std" | "core") {
                return None;
            }
            path.segments[1].ident.to_string()
        }
        _ => return None,
    };
    if name == "dbg" {
        None
    } else if DIVERGING_MACROS.contains(&name.as_str()) {
        Some(true)
    } else if OBSERVATION_MACROS
        .iter()
        .chain(crate::COMMA_BLIND_MACROS.iter())
        .any(|known| *known == name)
    {
        Some(false)
    } else {
        None
    }
}

/// The macro arguments of `mac` as expressions, empty when they do not
/// parse as a comma list or an `elem; count` repeat.
fn macro_args(mac: &syn::Macro) -> Vec<syn::Expr> {
    match syn::parse2::<MacroArgs>(mac.tokens.clone()) {
        Ok(MacroArgs::List(list)) => list.into_iter().collect(),
        Ok(MacroArgs::Repeat(list)) => list.into_iter().collect(),
        Err(_) => Vec::new(),
    }
}

/// Peel the parentheses of `expr` down to the innermost non-paren
/// expression.
fn peel_paren(expr: &syn::Expr) -> &syn::Expr {
    let mut expr = expr;
    while let syn::Expr::Paren(paren) = expr {
        expr = &paren.expr;
    }
    expr
}

/// Whether `op` is a compound assignment.
fn is_assign_op(op: &syn::BinOp) -> bool {
    matches!(
        op,
        syn::BinOp::AddAssign(_)
            | syn::BinOp::SubAssign(_)
            | syn::BinOp::MulAssign(_)
            | syn::BinOp::DivAssign(_)
            | syn::BinOp::RemAssign(_)
            | syn::BinOp::ShlAssign(_)
            | syn::BinOp::ShrAssign(_)
            | syn::BinOp::BitXorAssign(_)
            | syn::BinOp::BitAndAssign(_)
            | syn::BinOp::BitOrAssign(_)
    )
}

/// The total result type of a bitwise or shift operation over proven scalars.
fn total_op(bin_op: &syn::BinOp, left: &Lbl, right: &Lbl) -> Option<PrimTy> {
    match bin_op {
        syn::BinOp::Shl(_) | syn::BinOp::Shr(_) => {
            let prim = left.prim()?;
            let PrimTy::Int { width, .. } = prim else {
                return None;
            };
            match right {
                Lbl::Int(count, _) if *count < u128::from(width) => Some(prim),
                _ => None,
            }
        }
        syn::BinOp::BitAnd(_) | syn::BinOp::BitOr(_) | syn::BinOp::BitXor(_) => {
            match (left.prim(), right.prim()) {
                (Some(left), Some(right)) if left == right => Some(left),
                (Some(left), None) if matches!(right, Lbl::Int(_, None)) => Some(left),
                (None, Some(right)) if matches!(left, Lbl::Int(_, None)) => Some(right),
                _ => None,
            }
        }
        _ => None,
    }
}

fn label_lit(lit: &syn::Lit) -> Lbl {
    match lit {
        syn::Lit::Int(integer) => {
            let prim = crate::scope::primitive_suffix(integer.suffix());
            match integer.base10_parse::<u128>() {
                Ok(value) => Lbl::Int(value, prim),
                Err(_) => Lbl::Other,
            }
        }
        syn::Lit::Bool(boolean) => Lbl::Bool(boolean.value),
        _ => Lbl::Other,
    }
}

/// The binding names a pattern introduces, in source order.
fn pat_names(pat: &syn::Pat) -> Vec<String> {
    match pat {
        syn::Pat::Ident(ident) => {
            let text = ident.ident.to_string();
            vec![unraw(&text).to_string()]
        }
        syn::Pat::Tuple(tuple) => tuple.elems.iter().flat_map(pat_names).collect(),
        syn::Pat::TupleStruct(tuple_struct) => {
            tuple_struct.elems.iter().flat_map(pat_names).collect()
        }
        syn::Pat::Struct(s) => s
            .fields
            .iter()
            .flat_map(|field| pat_names(&field.pat))
            .collect(),
        syn::Pat::Slice(slice) => slice.elems.iter().flat_map(pat_names).collect(),
        syn::Pat::Paren(paren) => pat_names(&paren.pat),
        syn::Pat::Reference(reference) => pat_names(&reference.pat),
        syn::Pat::Type(pat_type) => pat_names(&pat_type.pat),
        syn::Pat::Guard(guard) => pat_names(&guard.pat),
        syn::Pat::Or(orbit) => orbit.cases.first().map(pat_names).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The binding names of a pattern list, as used by for-loops and closures.
fn pat_names_list(pat: &syn::Pat) -> Vec<String> {
    match pat {
        syn::Pat::Tuple(tuple) => tuple.elems.iter().flat_map(pat_names).collect(),
        _ => pat_names(pat),
    }
}

/// The binding name of a clean immutable `let` pattern, when one is
/// proven.
fn let_name(local: &syn::Local) -> Option<String> {
    if !local.attrs.is_empty() {
        return None;
    }
    match &local.pat {
        syn::Pat::Ident(ident) if ident.subpat.is_none() && ident.mutability.is_none() => {
            let text = ident.ident.to_string();
            Some(unraw(&text).to_string())
        }
        _ => None,
    }
}

/// The binding name of a simple `let` pattern, mutable or not.
fn local_binding_name(local: &syn::Local) -> Option<String> {
    match &local.pat {
        syn::Pat::Ident(ident) if ident.subpat.is_none() => {
            let text = ident.ident.to_string();
            Some(unraw(&text).to_string())
        }
        _ => None,
    }
}

/// The assigned binding's name, when the target is a clean identifier.
fn assign_target(stmt: &syn::Stmt) -> Option<String> {
    let syn::Stmt::Expr(expr, _) = stmt else {
        return None;
    };
    let assign: &syn::Expr = match peel_paren(expr) {
        syn::Expr::Assign(assign) => &assign.left,
        syn::Expr::Binary(binary) if is_assign_op(&binary.op) => &binary.left,
        _ => return None,
    };
    match assign {
        syn::Expr::Path(path) if path.qself.is_none() && path.path.segments.len() == 1 => {
            let text = path.path.segments[0].ident.to_string();
            Some(unraw(&text).to_string())
        }
        _ => None,
    }
}

fn stmt_macro(stmt: &syn::Stmt) -> Option<&syn::Macro> {
    match stmt {
        syn::Stmt::Macro(mac) => Some(&mac.mac),
        syn::Stmt::Expr(syn::Expr::Macro(mac), _) => Some(&mac.mac),
        syn::Stmt::Item(syn::Item::Macro(mac)) => Some(&mac.mac),
        _ => None,
    }
}

/// Whether a statement expression is a control boundary: a `return`,
/// `break`, `continue`, or a top-level `?`.
fn stmt_boundary(expr: &syn::Expr) -> bool {
    matches!(
        peel_paren(expr),
        syn::Expr::Return(_) | syn::Expr::Break(_) | syn::Expr::Continue(_) | syn::Expr::Try(_)
    )
}

/// The statement's place in the block walk.
enum StmtKind {
    /// A known observation with modeled value inputs.
    Barrier,
    /// A control boundary, a possible divergence.
    Boundary(bool),
    /// Everything else, opaque syntax.
    Plain,
}

fn stmt_kind(renamer: &Renamer, stmt: &syn::Stmt) -> StmtKind {
    if let Some(mac) = stmt_macro(stmt) {
        let name = mac
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
            .unwrap_or_default();
        if renamer.macro_shadowed(unraw(&name)) {
            return StmtKind::Plain;
        }
        return match macro_kind(&mac.path) {
            Some(true) => StmtKind::Boundary(matches!(
                name.as_str(),
                "panic" | "todo" | "unimplemented" | "unreachable"
            )),
            Some(false) if OBSERVATION_MACROS.contains(&name.as_str()) => StmtKind::Barrier,
            _ => StmtKind::Plain,
        };
    }
    if let syn::Stmt::Expr(expr, _) = stmt
        && stmt_boundary(expr)
    {
        return StmtKind::Boundary(matches!(
            peel_paren(expr),
            syn::Expr::Return(_) | syn::Expr::Break(_) | syn::Expr::Continue(_)
        ));
    }
    StmtKind::Plain
}

/// The fact and sites an expression carries, nested blocks simulated
/// with cloned environments, borrows, addresses, receivers and captures
/// marked on the referenced producer's storage.
fn label_expr(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    expr: &syn::Expr,
) -> (Lbl, Vec<Site>) {
    match expr {
        syn::Expr::Lit(lit) => (label_lit(&lit.lit), Vec::new()),
        syn::Expr::Path(path) if path.qself.is_none() => label_path(env, &path.path),
        syn::Expr::Binary(bin) => label_binary(renamer, env, bin),
        syn::Expr::Paren(paren) => label_expr(renamer, env, &paren.expr),
        syn::Expr::Group(group) => label_expr(renamer, env, &group.expr),
        syn::Expr::Tuple(tuple) => label_elems(renamer, env, tuple.elems.iter()),
        syn::Expr::Call(call) => label_call(renamer, env, call),
        syn::Expr::MethodCall(method) => label_method(renamer, env, method),
        syn::Expr::Macro(mac) => {
            let args = macro_args(&mac.mac);
            label_elems(renamer, env, args.iter())
        }
        syn::Expr::Reference(reference) => label_storage(renamer, env, &reference.expr),
        syn::Expr::Unary(unary) => {
            let (_, sites) = label_expr(renamer, env, &unary.expr);
            (Lbl::Other, sites)
        }
        syn::Expr::Block(block) => {
            let (_, sites) = label_block(renamer, env, &block.block);
            (Lbl::Other, sites)
        }
        syn::Expr::If(r#if) => label_if(renamer, env, r#if),
        syn::Expr::ForLoop(for_loop) => label_for(renamer, env, for_loop),
        syn::Expr::While(r#while) => {
            let mut sites = label_sites(renamer, env, &r#while.cond);
            let (_, b_sites) = label_block(renamer, env, &r#while.body);
            sites.extend(b_sites);
            (Lbl::Other, sites)
        }
        syn::Expr::Loop(r#loop) => {
            let (_, sites) = label_block(renamer, env, &r#loop.body);
            (Lbl::Other, sites)
        }
        syn::Expr::Match(r#match) => label_match(renamer, env, r#match),
        syn::Expr::Closure(closure) => label_closure(renamer, env, closure),
        syn::Expr::Array(array) => label_elems(renamer, env, array.elems.iter()),
        syn::Expr::Repeat(repeat) => {
            let (_, mut sites) = label_expr(renamer, env, &repeat.expr);
            let (_, l_sites) = label_expr(renamer, env, &repeat.len);
            sites.extend(l_sites);
            (Lbl::Other, sites)
        }
        syn::Expr::Range(range) => label_range(renamer, env, range),
        syn::Expr::Index(index_expr) => {
            let (_, mut sites) = label_expr(renamer, env, &index_expr.expr);
            let (_, i_sites) = label_expr(renamer, env, &index_expr.index);
            sites.extend(i_sites);
            (Lbl::Other, sites)
        }
        syn::Expr::Field(field) => {
            let (_, sites) = label_expr(renamer, env, &field.base);
            (Lbl::Other, sites)
        }
        syn::Expr::Struct(struct_expr) => label_struct(renamer, env, struct_expr),
        syn::Expr::Let(let_expr) => {
            let (_, sites) = label_expr(renamer, env, &let_expr.expr);
            (Lbl::Other, sites)
        }
        syn::Expr::Assign(assign) => {
            let (_, mut sites) = label_expr(renamer, env, &assign.left);
            let (_, r_sites) = label_expr(renamer, env, &assign.right);
            sites.extend(r_sites);
            (Lbl::Other, sites)
        }
        syn::Expr::Return(r#return) => label_optional(renamer, env, r#return.expr.as_deref()),
        syn::Expr::Break(break_expr) => label_optional(renamer, env, break_expr.expr.as_deref()),
        syn::Expr::Try(try_expr) => {
            let (_, sites) = label_expr(renamer, env, &try_expr.expr);
            (Lbl::Other, sites)
        }
        syn::Expr::TryBlock(block) => {
            let (_, sites) = label_block(renamer, env, &block.block);
            (Lbl::Other, sites)
        }
        syn::Expr::Unsafe(block) => {
            let (_, sites) = label_block(renamer, env, &block.block);
            (Lbl::Other, sites)
        }
        syn::Expr::Async(block) => {
            let (_, sites) = label_block(renamer, env, &block.block);
            (Lbl::Other, sites)
        }
        syn::Expr::Const(block) => {
            let (_, sites) = label_block(renamer, env, &block.block);
            (Lbl::Other, sites)
        }
        syn::Expr::Await(await_expr) => {
            let (_, sites) = label_expr(renamer, env, &await_expr.base);
            (Lbl::Other, sites)
        }
        syn::Expr::Cast(cast) => {
            let (_, sites) = label_expr(renamer, env, &cast.expr);
            (Lbl::Other, sites)
        }
        syn::Expr::RawAddr(raw) => label_storage(renamer, env, &raw.expr),
        _ => (Lbl::Other, Vec::new()),
    }
}

fn label_call(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    call: &syn::ExprCall,
) -> (Lbl, Vec<Site>) {
    let mut sites = label_sites(renamer, env, &call.func);
    let (_, arguments) = label_elems(renamer, env, call.args.iter());
    sites.extend(arguments);
    (Lbl::Other, sites)
}

fn label_binary(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    bin: &syn::ExprBinary,
) -> (Lbl, Vec<Site>) {
    let (left, mut sites) = label_expr(renamer, env, &bin.left);
    let (right, right_sites) = label_expr(renamer, env, &bin.right);
    sites.extend(right_sites);
    let total = total_op(&bin.op, &left, &right);
    (
        total.map_or(Lbl::Other, |prim| Lbl::Value(Some(prim))),
        sites,
    )
}

fn label_method(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    method: &syn::ExprMethodCall,
) -> (Lbl, Vec<Site>) {
    let (_, mut sites) = label_storage(renamer, env, &method.receiver);
    let (_, arguments) = label_elems(renamer, env, method.args.iter());
    sites.extend(arguments);
    (Lbl::Other, sites)
}

fn label_storage(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    expr: &syn::Expr,
) -> (Lbl, Vec<Site>) {
    let (_, mut sites) = label_expr(renamer, env, expr);
    for site in &mut sites {
        site.storage = true;
    }
    (Lbl::Other, sites)
}

fn label_if(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    expr: &syn::ExprIf,
) -> (Lbl, Vec<Site>) {
    let mut sites = label_sites(renamer, env, &expr.cond);
    let (_, then_sites) = label_block(renamer, env, &expr.then_branch);
    sites.extend(then_sites);
    if let Some((_, branch)) = &expr.else_branch {
        sites.extend(label_sites(renamer, env, branch));
    }
    (Lbl::Other, sites)
}

fn label_for(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    expr: &syn::ExprForLoop,
) -> (Lbl, Vec<Site>) {
    let mut sites = label_sites(renamer, env, &expr.expr);
    let mut inner = env.clone();
    for name in pat_names_list(&expr.pat) {
        inner.insert(
            name,
            Value {
                producer: None,
                prim: None,
            },
        );
    }
    let (_, body_sites) = label_block(renamer, &inner, &expr.body);
    sites.extend(body_sites);
    (Lbl::Other, sites)
}

fn label_match(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    expr: &syn::ExprMatch,
) -> (Lbl, Vec<Site>) {
    let mut sites = label_sites(renamer, env, &expr.expr);
    for arm in &expr.arms {
        let mut inner = env.clone();
        for name in pat_names_list(&arm.pat) {
            inner.insert(
                name,
                Value {
                    producer: None,
                    prim: None,
                },
            );
        }
        if let syn::Pat::Guard(guard) = &arm.pat {
            sites.extend(label_sites(renamer, &inner, &guard.guard));
        }
        sites.extend(label_sites(renamer, &inner, &arm.body));
    }
    (Lbl::Other, sites)
}

fn label_closure(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    expr: &syn::ExprClosure,
) -> (Lbl, Vec<Site>) {
    let mut inner = env.clone();
    for input in &expr.inputs {
        let prim = match input {
            syn::Pat::Type(pat_type) => renamer.param_prim(&pat_type.ty),
            _ => None,
        };
        for name in pat_names_list(input) {
            inner.insert(
                name,
                Value {
                    producer: None,
                    prim,
                },
            );
        }
    }
    let (_, mut sites) = match &*expr.body {
        syn::Expr::Block(block) => label_block(renamer, &inner, &block.block),
        other => label_expr(renamer, &inner, other),
    };
    for site in &mut sites {
        site.storage = true;
    }
    (Lbl::Other, sites)
}

fn label_range(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    expr: &syn::ExprRange,
) -> (Lbl, Vec<Site>) {
    let mut sites = Vec::new();
    if let Some(start) = &expr.start {
        sites.extend(label_sites(renamer, env, start));
    }
    if let Some(end) = &expr.end {
        sites.extend(label_sites(renamer, env, end));
    }
    (Lbl::Other, sites)
}

fn label_struct(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    expr: &syn::ExprStruct,
) -> (Lbl, Vec<Site>) {
    let (_, mut sites) = label_path(env, &expr.path);
    for field in &expr.fields {
        sites.extend(label_sites(renamer, env, &field.expr));
    }
    (Lbl::Other, sites)
}

fn label_optional(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    expr: Option<&syn::Expr>,
) -> (Lbl, Vec<Site>) {
    (
        Lbl::Other,
        expr.map_or_else(Vec::new, |expr| label_sites(renamer, env, expr)),
    )
}

/// The sites of a list of expressions, in source order.
fn label_elems<'a>(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    elems: impl Iterator<Item = &'a syn::Expr>,
) -> (Lbl, Vec<Site>) {
    let mut sites = Vec::new();
    for elem in elems {
        let (_, e_sites) = label_expr(renamer, env, elem);
        sites.extend(e_sites);
    }
    (Lbl::Other, sites)
}

/// The sites of one expression.
fn label_sites(renamer: &Renamer, env: &BTreeMap<String, Value>, expr: &syn::Expr) -> Vec<Site> {
    let (_, sites) = label_expr(renamer, env, expr);
    sites
}

/// The label and sites of a path, a single-segment unqualified name
/// resolved through the environment.
fn label_path(env: &BTreeMap<String, Value>, path: &syn::Path) -> (Lbl, Vec<Site>) {
    if path.segments.len() == 1 {
        let name = path.segments[0].ident.to_string();
        return match env.get(unraw(&name)) {
            Some(value) => (
                Lbl::Value(value.prim),
                vec![Site {
                    producer: value.producer,
                    storage: false,
                }],
            ),
            None => (Lbl::Value(None), Vec::new()),
        };
    }
    (Lbl::Other, Vec::new())
}

/// The label of a nested block: its `let` bindings extend the cloned
/// environment, its references carry as sites.
fn label_block(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    block: &syn::Block,
) -> (Lbl, Vec<Site>) {
    let mut inner = env.clone();
    let mut sites = Vec::new();
    for stmt in &block.stmts {
        let (_, stmt_sites) = statement_label(renamer, &inner, stmt);
        sites.extend(stmt_sites);
        if let syn::Stmt::Local(local) = stmt
            && let Some(name) = local_binding_name(local)
        {
            let prim = if let_name(local).is_some() {
                match &local.init {
                    Some(init) if init.diverge.is_none() => {
                        label_expr(renamer, &inner, &init.expr).0.prim()
                    }
                    _ => None,
                }
            } else {
                None
            };
            inner.insert(
                name,
                Value {
                    producer: None,
                    prim,
                },
            );
        }
    }
    (Lbl::Other, sites)
}

/// The sites a statement carries.
fn statement_label(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    stmt: &syn::Stmt,
) -> (Lbl, Vec<Site>) {
    match stmt {
        syn::Stmt::Expr(expr, _) => label_expr(renamer, env, expr),
        syn::Stmt::Macro(stmt_macro) => {
            let args = macro_args(&stmt_macro.mac);
            label_elems(renamer, env, args.iter())
        }
        syn::Stmt::Local(local) => match local.init.as_ref() {
            Some(init) if init.diverge.is_none() => label_expr(renamer, env, &init.expr),
            _ => (Lbl::Other, Vec::new()),
        },
        syn::Stmt::Item(_) => (Lbl::Other, Vec::new()),
    }
}

/// The name, the proven initializer and the sites a producer `let`
/// statement carries, `None` when the statement does not prove one.
fn let_producer(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    local: &syn::Local,
) -> Option<(String, Lbl, Vec<Site>)> {
    let name = let_name(local)?;
    let init = local.init.as_ref()?;
    if init.diverge.is_some() {
        return None;
    }
    let (label, sites) = label_expr(renamer, env, &init.expr);
    Some((name, label, sites))
}

/// The proven initializer type of `expr`, when it stays inside the total
/// primitive language.
fn total_init(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    expr: &syn::Expr,
) -> Option<PrimTy> {
    label_expr(renamer, env, expr).0.prim()
}

/// Whether the tail root of `expr` is a clean read of proven values: a
/// proven literal, a proven reference, or a total expression over them,
/// a tuple of such.
fn tail_root(renamer: &Renamer, env: &BTreeMap<String, Value>, expr: &syn::Expr) -> bool {
    match peel_paren(expr) {
        syn::Expr::Tuple(tuple) => tuple
            .elems
            .iter()
            .all(|element| tail_root(renamer, env, element)),
        _ => total_init(renamer, env, expr).is_some(),
    }
}
