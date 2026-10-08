//! Exact, bounded scheduling of proven scalar declaration regions.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use quote::ToTokens as _;
use syn::visit::Visit;

use crate::alpha::{MacroArgs, Renamer};
use crate::reference::{BlockProof, ReferenceFacts};
use crate::scope::{FreeKey, PrimTy, unraw};

/// The graph nodes a region may still be labeled with.
const MAX_REGION_NODES: usize = 16_384;
/// The block nesting level a function scope may analyze.
const MAX_BLOCK_DEPTH: usize = 64;
/// The expanded search states a region may spend, partial
/// individualizations counted, refinement rounds never.
const MAX_LABEL_STATES: usize = 4_096;

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

/// The consumer-use axis a region's labeling records on.
const USE_DOWN: u8 = 0;
const USE_OPAQUE: u8 = 1;
const USE_BOUNDARY: u8 = 2;
const USE_UNREACHABLE: u8 = 3;
const USE_OBSERVED: u8 = 4;
const USE_CAPTURE: u8 = 5;
const USE_TAIL: u8 = 6;

/// A value reference the labels name by.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Port {
    /// A function parameter, by declared position.
    Param(usize),
    /// A closure input, by its canonical binding position.
    Closure(usize),
    /// A binder local to the analysis walk.
    Local(usize),
    /// A producer of the block being analyzed, by its index.
    Node(usize),
    /// A sealed producer of an earlier region, by region sequence and final slot.
    Sealed(usize, usize),
    /// A name the shared resolver identifies.
    Free(FreeKey),
}

/// A value reference with its proven scalar type.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct Value {
    pub(crate) port: Port,
    pub(crate) prim: Option<PrimTy>,
}

/// A labeled node of the expression language the analysis reads.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Lbl {
    /// An integer literal, `None` when the suffix leaves the type unproven.
    Int(u128, Option<PrimTy>),
    /// A boolean literal.
    Bool(bool),
    /// Any other literal, by its text.
    Lit(String),
    /// A value reference.
    Ref(Value),
    /// A total bitwise or valid shift over proven scalars.
    Op(char, Box<Lbl>, Box<Lbl>, PrimTy),
    /// Any other expression, by a kind tag and its ordered children.
    Other(String, Vec<Lbl>),
    /// A first-reference index in a projected future-producer graph.
    Future(usize, Option<PrimTy>),
    /// An ordered root and its sharing-preserving future definitions.
    Graph(Box<Lbl>, Vec<Lbl>),
}

impl Lbl {
    /// The proven scalar type of a total expression, `None` otherwise.
    fn prim(&self) -> Option<PrimTy> {
        match self {
            Self::Int(_, prim) => *prim,
            Self::Bool(_) => Some(PrimTy::Bool),
            Self::Ref(value) => value.prim,
            Self::Op(_, _, _, prim) => Some(*prim),
            _ => None,
        }
    }
}

/// The context a reference site holds: a value read or a storage
/// observation of the referenced producer's declaration.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    /// A value read of a stable producer.
    Value,
    /// A borrow, address, receiver or capture observation of its storage.
    Storage,
}

/// A reference site of a labeled expression.
#[derive(Clone)]
struct Site {
    port: Port,
    ctx: Ctx,
}

/// A consumer use of a block producer, the consumer side of the region's
/// labeling.
#[derive(Clone)]
struct Use {
    /// The producer's index.
    node: usize,
    /// The consumer-use axis.
    kind: u8,
    /// The consumer's statement index, the tail element position, or zero.
    index: usize,
    /// The consumer expression's structural label.
    label: Lbl,
}

/// A producer of the block being analyzed.
struct Producer {
    /// The statement index in the block.
    stmt: usize,
    /// The region the producer belongs to.
    region: usize,
    /// The region slot, position among its region's producers.
    slot: usize,
    /// The binding's name, raw flag dropped.
    name: String,
    /// The proven initializer.
    init: Lbl,
    /// The proven type.
    prim: PrimTy,
    /// The same-region producers this one reads, by slot, in operand order.
    deps: Vec<usize>,
}

/// A run of the block's statements that schedules together.
#[derive(Debug)]
struct Region {
    /// The region's producers, producer indices in source order.
    producers: Vec<usize>,
    /// The first statement in the region.
    start: usize,
    /// A region retained as opaque, by bound or by taint.
    dead: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ScopeState {
    /// A frozen scope: an unknown observer reached anywhere in the body.
    pub(crate) frozen: bool,
    /// The running region sequence, scoped to the function.
    pub(crate) seq: usize,
    /// The running local ports, scoped to the function.
    pub(crate) next_local: usize,
}

pub(crate) struct BindingPlan {
    pub(crate) offset: isize,
    pub(crate) prim: Option<PrimTy>,
    pub(crate) port: Port,
}

pub(crate) struct BlockFacts {
    plans: BTreeMap<*const syn::Pat, BindingPlan>,
    permutation: Vec<usize>,
}

impl BlockFacts {
    pub(crate) fn plan(&self, pat: &syn::Pat) -> Option<&BindingPlan> {
        self.plans.get(&core::ptr::from_ref(pat))
    }

    // Cycle-follow the transposition in place, so the loop keeps its own index.
    pub(crate) fn finish(mut self, block: &mut syn::Block, facts: &mut ReferenceFacts) {
        for position in 0..self.permutation.len() {
            while self.permutation[position] != position {
                let destination = self.permutation[position];
                block.stmts.swap(position, destination);
                facts.swap_block(block, position, destination);
                self.permutation.swap(position, destination);
            }
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

/// The names a format string captures or uses as width or precision that
/// are not among its arguments.
fn format_refs(format: &str, args: &[syn::Expr]) -> Vec<String> {
    let mut names = Vec::new();
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '{' {
            continue;
        }
        if chars.peek() == Some(&'{') {
            chars.next();
            continue;
        }
        let mut name = String::new();
        let mut spec = String::new();
        let mut in_spec = false;
        for c in chars.by_ref() {
            match c {
                '}' => break,
                ':' if !in_spec => in_spec = true,
                _ if in_spec => spec.push(c),
                _ if c.is_alphanumeric() || c == '_' => name.push(c),
                _ => {}
            }
        }
        if !name.is_empty() {
            names.push(name);
        }
        for (position, c) in spec.char_indices() {
            if c != '$' {
                continue;
            }
            let before = &spec[..position];
            let start = before
                .char_indices()
                .rev()
                .find(|(_, c)| !c.is_alphanumeric() && *c != '_')
                .map_or(0, |(index, c)| index + c.len_utf8());
            if start < before.len() {
                names.push(before[start..].to_string());
            }
        }
    }
    names
        .into_iter()
        .filter(|name| !args.iter().any(|arg| is_arg_path(arg, name)))
        .collect()
}

fn is_arg_path(arg: &syn::Expr, name: &str) -> bool {
    matches!(arg, syn::Expr::Assign(assign)
        if matches!(&*assign.left, syn::Expr::Path(path)
            if path.qself.is_none() && path.path.is_ident(name)))
}

struct UnknownScan<'a, 'env> {
    renamer: &'a Renamer<'env>,
    found: bool,
}

impl UnknownScan<'_, '_> {
    /// Whether the macro call's origin cannot be proven: an unmodeled
    /// name, or an unqualified name the shared resolver shadows.
    fn uncertain(&self, mac: &syn::Macro) -> bool {
        if macro_kind(&mac.path).is_none() {
            return true;
        }
        let head = mac.path.segments[0].ident.to_string();
        if mac.path.segments.len() == 1 {
            self.renamer.macro_shadowed(unraw(&head))
        } else {
            self.renamer.macro_shadowed(unraw(&head))
                || matches!(
                    self.renamer.type_identity(unraw(&head)),
                    FreeKey::Resolved(_)
                )
        }
    }
}

impl<'ast> Visit<'ast> for UnknownScan<'_, '_> {
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

/// Whether the function body of `block` reaches a macro call the analysis
/// does not model, a nested function body excluded as its own scope.
pub(crate) fn unknown_macro_reaches(renamer: &Renamer, block: &syn::Block) -> bool {
    let mut scan = UnknownScan {
        renamer,
        found: false,
    };
    syn::visit::visit_block(&mut scan, block);
    scan.found
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

/// Whether `op` is a compound assignment the labels group under one tag.
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

/// The kind tag of a binary operator the labels keep distinct.
fn op_text(op: &syn::BinOp) -> String {
    match op {
        syn::BinOp::Add(_) => "+".into(),
        syn::BinOp::Sub(_) => "-".into(),
        syn::BinOp::Mul(_) => "*".into(),
        syn::BinOp::Div(_) => "/".into(),
        syn::BinOp::Rem(_) => "%".into(),
        syn::BinOp::Shl(_) => "shl".into(),
        syn::BinOp::Shr(_) => "shr".into(),
        syn::BinOp::BitXor(_) => "xor".into(),
        syn::BinOp::BitAnd(_) => "and".into(),
        syn::BinOp::BitOr(_) => "or".into(),
        syn::BinOp::Eq(_) => "eq".into(),
        syn::BinOp::Lt(_) => "lt".into(),
        syn::BinOp::Le(_) => "le".into(),
        syn::BinOp::Gt(_) => "gt".into(),
        syn::BinOp::Ge(_) => "ge".into(),
        syn::BinOp::Ne(_) => "ne".into(),
        syn::BinOp::And(_) => "&&".into(),
        syn::BinOp::Or(_) => "||".into(),
        _ => "op".into(),
    }
}

/// The total result type of a bitwise or shift operation over proven
/// scalars, an unsuffixed integer literal adopting the operation's
/// established type.
fn total_op(bin_op: &syn::BinOp, left: &Lbl, right: &Lbl) -> Option<(char, PrimTy)> {
    let op = match bin_op {
        syn::BinOp::BitAnd(_) => '&',
        syn::BinOp::BitOr(_) => '|',
        syn::BinOp::BitXor(_) => '^',
        syn::BinOp::Shl(_) => '<',
        syn::BinOp::Shr(_) => '>',
        _ => return None,
    };
    let left_prim = left.prim();
    let right_prim = right.prim();
    if matches!(bin_op, syn::BinOp::Shl(_) | syn::BinOp::Shr(_)) {
        let prim = left_prim?;
        let PrimTy::Int { width, .. } = prim else {
            return None;
        };
        return match right {
            Lbl::Int(count, _) if *count < u128::from(width.bits()) => Some((op, prim)),
            _ => None,
        };
    }
    let prim = match (left_prim, right_prim) {
        (Some(left), Some(right)) if left == right => left,
        (Some(left), None) if matches!(right, Lbl::Int(_, None)) => left,
        (None, Some(right)) if matches!(left, Lbl::Int(_, None)) => right,
        _ => return None,
    };
    Some((op, prim))
}

/// The text of `path`, the segments joined by `::`.
fn path_text(path: &syn::Path) -> String {
    let mut text = String::new();
    if path.leading_colon.is_some() {
        text.push_str("::");
    }
    for (index, segment) in path.segments.iter().enumerate() {
        if index > 0 {
            text.push_str("::");
        }
        text.push_str(&segment.ident.to_string());
        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
            text.push('<');
            for (index, arg) in args.args.iter().enumerate() {
                if index > 0 {
                    text.push_str(", ");
                }
                match arg {
                    syn::GenericArgument::Type(ty) => {
                        if let syn::Type::Path(ty_path) = ty {
                            text.push_str(&path_text(&ty_path.path));
                        } else {
                            text.push('?');
                        }
                    }
                    _ => text.push('?'),
                }
            }
            text.push('>');
        }
    }
    text
}

fn label_lit(lit: &syn::Lit) -> Lbl {
    match lit {
        syn::Lit::Int(integer) => {
            let prim = crate::scope::primitive_suffix(integer.suffix());
            match integer.base10_parse::<u128>() {
                Ok(value) => Lbl::Int(value, prim),
                Err(_) => Lbl::Lit(integer.to_token_stream().to_string()),
            }
        }
        syn::Lit::Bool(boolean) => Lbl::Bool(boolean.value),
        other => Lbl::Lit(other.to_token_stream().to_string()),
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

/// The statement's place in the region walk.
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

/// The label and reference sites an expression carries. Nested blocks,
/// loops, arms and closure bodies extend the environment with their local
/// binders; borrows, addresses, receivers and captures mark the sites on
/// the storage of the referenced producers.
fn label_expr(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    expr: &syn::Expr,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    match expr {
        syn::Expr::Lit(lit) => (label_lit(&lit.lit), Vec::new()),
        syn::Expr::Path(path) if path.qself.is_none() => label_path(renamer, env, &path.path),
        syn::Expr::Path(_) => (Lbl::Other("q".into(), Vec::new()), Vec::new()),
        syn::Expr::Binary(bin) => label_binary(renamer, env, bin, consumer, locals),
        syn::Expr::Paren(paren) => label_expr(renamer, env, &paren.expr, consumer, locals),
        syn::Expr::Group(group) => label_expr(renamer, env, &group.expr, consumer, locals),
        syn::Expr::Tuple(tuple) => label_tuple(renamer, env, &tuple.elems, consumer, locals),
        syn::Expr::Call(call) => label_call(renamer, env, call, consumer, locals),
        syn::Expr::MethodCall(method) => label_method(renamer, env, method, consumer, locals),
        syn::Expr::Macro(mac) => macro_label(renamer, env, &mac.mac, consumer, locals),
        syn::Expr::Reference(reference) => {
            label_reference(renamer, env, reference, consumer, locals)
        }
        syn::Expr::Unary(unary) => label_unary(renamer, env, unary, consumer, locals),
        syn::Expr::Block(block) => label_block(renamer, env, &block.block, consumer, locals),
        syn::Expr::If(r#if) => label_if_expr(renamer, env, r#if, consumer, locals),
        syn::Expr::ForLoop(for_loop) => label_for_loop(renamer, env, for_loop, consumer, locals),
        syn::Expr::While(r#while) => label_while(renamer, env, r#while, consumer, locals),
        syn::Expr::Loop(r#loop) => label_loop(renamer, env, r#loop, consumer, locals),
        syn::Expr::Match(r#match) => label_match_expr(renamer, env, r#match, consumer, locals),
        syn::Expr::Closure(closure) => label_closure(renamer, env, closure, consumer, locals),
        syn::Expr::Array(array) => label_array(renamer, env, array, consumer, locals),
        syn::Expr::Repeat(repeat) => label_repeat(renamer, env, repeat, consumer, locals),
        syn::Expr::Range(range) => label_range(renamer, env, range, consumer, locals),
        syn::Expr::Index(index_expr) => label_index(renamer, env, index_expr, consumer, locals),
        syn::Expr::Field(field) => label_field(renamer, env, field, consumer, locals),
        syn::Expr::Struct(struct_expr) => {
            label_struct_expr(renamer, env, struct_expr, consumer, locals)
        }
        syn::Expr::Let(let_expr) => label_let_expr(renamer, env, let_expr, consumer, locals),
        syn::Expr::Assign(assign) => label_assign(renamer, env, assign, consumer, locals),
        syn::Expr::Return(r#return) => label_optional_value(
            "ret",
            renamer,
            env,
            r#return.expr.as_deref(),
            consumer,
            locals,
        ),
        syn::Expr::Break(break_expr) => label_optional_value(
            "br",
            renamer,
            env,
            break_expr.expr.as_deref(),
            consumer,
            locals,
        ),
        syn::Expr::Try(try_expr) => label_try(renamer, env, try_expr, consumer, locals),
        syn::Expr::TryBlock(block) => label_blk(renamer, env, &block.block, consumer, locals),
        syn::Expr::Unsafe(block) => label_blk(renamer, env, &block.block, consumer, locals),
        syn::Expr::Async(block) => label_blk(renamer, env, &block.block, consumer, locals),
        syn::Expr::Const(block) => label_blk(renamer, env, &block.block, consumer, locals),
        syn::Expr::Verbatim(tokens) => (
            Lbl::Other("v".into(), vec![Lbl::Lit(tokens.to_string())]),
            Vec::new(),
        ),
        syn::Expr::Await(await_expr) => label_await(renamer, env, await_expr, consumer, locals),
        syn::Expr::Cast(cast) => label_cast(renamer, env, cast, consumer, locals),
        syn::Expr::RawAddr(raw) => label_raw_addr(renamer, env, raw, consumer, locals),
        _ => (Lbl::Other("e".into(), Vec::new()), Vec::new()),
    }
}

/// The label and sites of a path, a single-segment unqualified name
/// resolved through the environment.
fn label_path(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    path: &syn::Path,
) -> (Lbl, Vec<Site>) {
    if path.segments.len() == 1 {
        let name = path.segments[0].ident.to_string();
        return match env.get(unraw(&name)) {
            Some(value) => (
                Lbl::Ref(value.clone()),
                vec![Site {
                    port: value.port.clone(),
                    ctx: Ctx::Value,
                }],
            ),
            None => (
                Lbl::Ref(Value {
                    port: Port::Free(renamer.value_identity(unraw(&name))),
                    prim: None,
                }),
                Vec::new(),
            ),
        };
    }
    (
        Lbl::Other("q".into(), vec![Lbl::Lit(path_text(path))]),
        Vec::new(),
    )
}

/// The label of a binary operation, a total one proven on both sides, any
/// other one kept by operator text.
fn label_binary(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    bin: &syn::ExprBinary,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (left, mut sites) = label_expr(renamer, env, &bin.left, consumer, locals);
    let (right, r_sites) = label_expr(renamer, env, &bin.right, consumer, locals);
    sites.extend(r_sites);
    let total = total_op(&bin.op, &left, &right);
    if let Some((op, prim)) = total {
        (Lbl::Op(op, Box::new(left), Box::new(right), prim), sites)
    } else {
        let tag = if is_assign_op(&bin.op) {
            "ao".into()
        } else {
            op_text(&bin.op)
        };
        (Lbl::Other(tag, vec![left, right]), sites)
    }
}

/// The label of a tuple, its elements in source order.
fn label_tuple(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    elems: &syn::punctuated::Punctuated<syn::Expr, syn::token::Comma>,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let mut children = Vec::new();
    let mut sites = Vec::new();
    for element in elems {
        let (label, e_sites) = label_expr(renamer, env, element, consumer, locals);
        children.push(label);
        sites.extend(e_sites);
    }
    (Lbl::Other("t".into(), children), sites)
}

/// The label of a call, its callee and arguments in operand order.
fn label_call(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    call: &syn::ExprCall,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (callee, mut sites) = label_expr(renamer, env, &call.func, consumer, locals);
    let mut children = vec![callee];
    for arg in &call.args {
        let (label, a_sites) = label_expr(renamer, env, arg, consumer, locals);
        children.push(label);
        sites.extend(a_sites);
    }
    (Lbl::Other("c".into(), children), sites)
}

/// The label of a method call, the receiver's sites marked on its storage,
/// its arguments in operand order.
fn label_method(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    method: &syn::ExprMethodCall,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (receiver, receiver_sites) = label_expr(renamer, env, &method.receiver, consumer, locals);
    let mut sites: Vec<_> = receiver_sites
        .into_iter()
        .map(|site| Site {
            ctx: Ctx::Storage,
            ..site
        })
        .collect();
    let mut children = vec![Lbl::Lit(method.method.to_string()), receiver];
    for arg in &method.args {
        let (label, a_sites) = label_expr(renamer, env, arg, consumer, locals);
        children.push(label);
        sites.extend(a_sites);
    }
    (Lbl::Other("m".into(), children), sites)
}

/// The label of a reference, its inner sites marked on the referenced
/// producer's storage.
fn label_reference(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    reference: &syn::ExprReference,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (inner, inner_sites) = label_expr(renamer, env, &reference.expr, consumer, locals);
    let sites = inner_sites
        .into_iter()
        .map(|site| Site {
            ctx: Ctx::Storage,
            ..site
        })
        .collect();
    let kind = if reference.mutability.is_some() {
        "rm"
    } else {
        "r"
    };
    (Lbl::Other(kind.into(), vec![inner]), sites)
}

/// The label of a unary expression, its operand labeled.
fn label_unary(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    unary: &syn::ExprUnary,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (inner, sites) = label_expr(renamer, env, &unary.expr, consumer, locals);
    (Lbl::Other("u".into(), vec![inner]), sites)
}

/// The label of an `if` expression, the else branch present when one is.
fn label_if_expr(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    r#if: &syn::ExprIf,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (cond, mut sites) = label_cond(renamer, env, &r#if.cond, consumer, locals);
    let (then, then_sites) = label_block(renamer, env, &r#if.then_branch, consumer, locals);
    let mut children = vec![cond, then];
    sites.extend(then_sites);
    if let Some((_, else_expr)) = &r#if.else_branch {
        let (else_label, else_sites) = label_expr(renamer, env, else_expr, consumer, locals);
        sites.extend(else_sites);
        children.push(else_label);
    }
    (Lbl::Other("if".into(), children), sites)
}

/// The label of a `for` loop, its iterable bound to a fresh local port.
fn label_for_loop(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    for_loop: &syn::ExprForLoop,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (iterable, mut sites) = label_expr(renamer, env, &for_loop.expr, consumer, locals);
    let mut inner = env.clone();
    for name in pat_names(&for_loop.pat) {
        inner.insert(
            name,
            Value {
                port: Port::Local(*locals),
                prim: None,
            },
        );
        *locals += 1;
    }
    let (body, body_sites) = label_block(renamer, &inner, &for_loop.body, consumer, locals);
    sites.extend(body_sites);
    (Lbl::Other("for".into(), vec![iterable, body]), sites)
}

/// The label of a `while` loop.
fn label_while(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    r#while: &syn::ExprWhile,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (cond, mut sites) = label_cond(renamer, env, &r#while.cond, consumer, locals);
    let (body, body_sites) = label_block(renamer, env, &r#while.body, consumer, locals);
    sites.extend(body_sites);
    (Lbl::Other("while".into(), vec![cond, body]), sites)
}

/// The label of a `loop` expression.
fn label_loop(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    r#loop: &syn::ExprLoop,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (body, sites) = label_block(renamer, env, &r#loop.body, consumer, locals);
    (Lbl::Other("loop".into(), vec![body]), sites)
}

/// The label of a `match` expression, each arm's pattern bindings fresh
/// local ports before its guard and body.
fn label_match_expr(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    r#match: &syn::ExprMatch,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (scrutinee, mut sites) = label_expr(renamer, env, &r#match.expr, consumer, locals);
    let mut children = vec![scrutinee];
    for arm in &r#match.arms {
        let mut inner = env.clone();
        for name in pat_names(&arm.pat) {
            inner.insert(
                name,
                Value {
                    port: Port::Local(*locals),
                    prim: None,
                },
            );
            *locals += 1;
        }
        let (guard_label, guard_sites) = match &arm.pat {
            syn::Pat::Guard(guard) => label_expr(renamer, &inner, &guard.guard, consumer, locals),
            _ => (Lbl::Other("ng".into(), Vec::new()), Vec::new()),
        };
        sites.extend(guard_sites);
        let (body, body_sites) = label_expr(renamer, &inner, &arm.body, consumer, locals);
        sites.extend(body_sites);
        children.push(Lbl::Other(
            "a".into(),
            vec![Lbl::Lit("P".into()), guard_label, body],
        ));
    }
    (Lbl::Other("match".into(), children), sites)
}

/// The label of a closure, its inputs fresh local ports with their proven
/// types, the body marked on its storage.
fn label_closure(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    closure: &syn::ExprClosure,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let mut inner = env.clone();
    for input in &closure.inputs {
        let prim = match input {
            syn::Pat::Type(pat_type) => renamer.param_prim(&pat_type.ty),
            _ => None,
        };
        for name in pat_names(input) {
            inner.insert(
                name,
                Value {
                    port: Port::Local(*locals),
                    prim,
                },
            );
            *locals += 1;
        }
    }
    let (body, body_sites) = match &*closure.body {
        syn::Expr::Block(block) => label_block(renamer, &inner, &block.block, consumer, locals),
        other => label_expr(renamer, &inner, other, consumer, locals),
    };
    let sites = body_sites
        .into_iter()
        .map(|site| Site {
            ctx: Ctx::Storage,
            ..site
        })
        .collect();
    (Lbl::Other("cl".into(), vec![body]), sites)
}

/// The label of an array literal, its elements in source order.
fn label_array(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    array: &syn::ExprArray,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let mut children = Vec::new();
    let mut sites = Vec::new();
    for element in &array.elems {
        let (label, e_sites) = label_expr(renamer, env, element, consumer, locals);
        children.push(label);
        sites.extend(e_sites);
    }
    (Lbl::Other("arr".into(), children), sites)
}

/// The label of a repeat expression, its element and its length.
fn label_repeat(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    repeat: &syn::ExprRepeat,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (element, mut sites) = label_expr(renamer, env, &repeat.expr, consumer, locals);
    let (length, l_sites) = label_expr(renamer, env, &repeat.len, consumer, locals);
    sites.extend(l_sites);
    (Lbl::Other("rep".into(), vec![element, length]), sites)
}

/// The label of a range expression, its endpoints present when they are.
fn label_range(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    range: &syn::ExprRange,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let mut children = Vec::new();
    let mut sites = Vec::new();
    if let Some(start) = &range.start {
        let (label, s_sites) = label_expr(renamer, env, start, consumer, locals);
        children.push(label);
        sites.extend(s_sites);
    }
    if let Some(end) = &range.end {
        let (label, s_sites) = label_expr(renamer, env, end, consumer, locals);
        children.push(label);
        sites.extend(s_sites);
    }
    (Lbl::Other("range".into(), children), sites)
}

/// The label of an index expression, its base and its index.
fn label_index(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    index_expr: &syn::ExprIndex,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (base, mut sites) = label_expr(renamer, env, &index_expr.expr, consumer, locals);
    let (idx, i_sites) = label_expr(renamer, env, &index_expr.index, consumer, locals);
    sites.extend(i_sites);
    (Lbl::Other("idx".into(), vec![base, idx]), sites)
}

/// The label of a field projection, its base and the member's text.
fn label_field(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    field: &syn::ExprField,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (base, sites) = label_expr(renamer, env, &field.base, consumer, locals);
    (
        Lbl::Other(
            "f".into(),
            vec![base, Lbl::Lit(field.member.to_token_stream().to_string())],
        ),
        sites,
    )
}

/// The label of a struct expression, its fields in source order.
fn label_struct_expr(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    struct_expr: &syn::ExprStruct,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let mut children = vec![Lbl::Lit(path_text(&struct_expr.path))];
    let mut sites = Vec::new();
    for field in &struct_expr.fields {
        let (label, f_sites) = label_expr(renamer, env, &field.expr, consumer, locals);
        children.push(label);
        sites.extend(f_sites);
    }
    (Lbl::Other("st".into(), children), sites)
}

/// The label of a `let` expression, its scrutinee labeled.
fn label_let_expr(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    let_expr: &syn::ExprLet,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (value, sites) = label_expr(renamer, env, &let_expr.expr, consumer, locals);
    (Lbl::Other("le".into(), vec![value]), sites)
}

/// The label of an assignment expression, its sides in operand order.
fn label_assign(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    assign: &syn::ExprAssign,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (left, mut sites) = label_expr(renamer, env, &assign.left, consumer, locals);
    let (right, r_sites) = label_expr(renamer, env, &assign.right, consumer, locals);
    sites.extend(r_sites);
    (Lbl::Other("as".into(), vec![left, right]), sites)
}

/// The label of an expression that may carry a value, a `return` or a
/// `break`, the operand empty when the value is absent.
fn label_optional_value(
    tag: &str,
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    value: Option<&syn::Expr>,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    match value {
        Some(value) => {
            let (label, sites) = label_expr(renamer, env, value, consumer, locals);
            (Lbl::Other(tag.into(), vec![label]), sites)
        }
        None => (Lbl::Other(tag.into(), Vec::new()), Vec::new()),
    }
}

/// The label of a `try` expression, its operand labeled.
fn label_try(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    try_expr: &syn::ExprTry,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (label, sites) = label_expr(renamer, env, &try_expr.expr, consumer, locals);
    (Lbl::Other("try".into(), vec![label]), sites)
}

/// The label of a block expression, a `try` block, an `unsafe` block, an
/// `async` block, or a `const` block.
fn label_blk(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    block: &syn::Block,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (label, sites) = label_block(renamer, env, block, consumer, locals);
    (Lbl::Other("blk".into(), vec![label]), sites)
}

/// The label of an `await` expression, its operand labeled.
fn label_await(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    await_expr: &syn::ExprAwait,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (label, sites) = label_expr(renamer, env, &await_expr.base, consumer, locals);
    (Lbl::Other("aw".into(), vec![label]), sites)
}

/// The label of a cast expression, its operand and its target type text.
fn label_cast(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    cast: &syn::ExprCast,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (value, sites) = label_expr(renamer, env, &cast.expr, consumer, locals);
    let ty = match &*cast.ty {
        syn::Type::Path(path) if path.qself.is_none() => {
            if path.path.segments.len() == 1 {
                let name = path.path.segments[0].ident.to_string();
                renamer.type_identity(unraw(&name)).text()
            } else {
                path_text(&path.path)
            }
        }
        _ => "?".into(),
    };
    (Lbl::Other("cast".into(), vec![value, Lbl::Lit(ty)]), sites)
}

/// The label of a raw pointer expression, its inner sites marked on the
/// referenced producer's storage.
fn label_raw_addr(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    raw: &syn::ExprRawAddr,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let (label, inner_sites) = label_expr(renamer, env, &raw.expr, consumer, locals);
    let sites = inner_sites
        .into_iter()
        .map(|site| Site {
            ctx: Ctx::Storage,
            ..site
        })
        .collect();
    (Lbl::Other("ra".into(), vec![label]), sites)
}

/// The label and reference sites of a macro call, its arguments labeled.
fn macro_label(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    mac: &syn::Macro,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let name = mac
        .path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
        .unwrap_or_default();
    let args = macro_args(mac);
    let mut children = Vec::new();
    let mut sites = Vec::new();
    for arg in &args {
        let (label, a_sites) = label_expr(renamer, env, arg, consumer, locals);
        children.push(label);
        sites.extend(a_sites);
    }
    (Lbl::Other(alloc::format!("m:{name}"), children), sites)
}

/// The label of a nested block: its statements in source order, its `let`
/// bindings extending the environment with local ports and a proven type,
/// its references carried as sites.
fn label_block(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    block: &syn::Block,
    _consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    let mut inner = env.clone();
    let mut children = Vec::new();
    let mut sites = Vec::new();
    for (position, stmt) in block.stmts.iter().enumerate() {
        let (label, stmt_sites) = statement_label(renamer, &inner, stmt, position, locals);
        sites.extend(stmt_sites);
        if let syn::Stmt::Local(local) = stmt
            && let Some(name) = local_binding_name(local)
        {
            let prim = if let_name(local).is_some() {
                label.prim()
            } else {
                None
            };
            inner.insert(
                name,
                Value {
                    port: Port::Local(*locals),
                    prim,
                },
            );
            *locals += 1;
        }
        children.push(label);
    }
    (Lbl::Other("B".into(), children), sites)
}

/// The label and the reference sites a statement carries.
fn statement_label(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    stmt: &syn::Stmt,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    match stmt {
        syn::Stmt::Expr(expr, _) => label_expr(renamer, env, expr, consumer, locals),
        syn::Stmt::Macro(stmt_macro) => {
            macro_label(renamer, env, &stmt_macro.mac, consumer, locals)
        }
        syn::Stmt::Local(local) => match local.init.as_ref() {
            Some(init) if init.diverge.is_none() => {
                label_expr(renamer, env, &init.expr, consumer, locals)
            }
            _ => (Lbl::Other("s".into(), Vec::new()), Vec::new()),
        },
        syn::Stmt::Item(_) => (Lbl::Other("s".into(), Vec::new()), Vec::new()),
    }
}

/// The label and sites of an `if` or `while` condition, a `let` pattern
/// named before the condition continues.
fn label_cond(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    cond: &syn::Expr,
    consumer: usize,
    locals: &mut usize,
) -> (Lbl, Vec<Site>) {
    match cond {
        syn::Expr::Binary(binary) if matches!(binary.op, syn::BinOp::And(_)) => {
            let (left, mut sites) = label_cond(renamer, env, &binary.left, consumer, locals);
            let (right, r_sites) = label_cond(renamer, env, &binary.right, consumer, locals);
            sites.extend(r_sites);
            (Lbl::Other("&&".into(), vec![left, right]), sites)
        }
        syn::Expr::Let(let_expr) => {
            let (value, sites) = label_expr(renamer, env, &let_expr.expr, consumer, locals);
            (Lbl::Other("lc".into(), vec![value]), sites)
        }
        other => label_expr(renamer, env, other, consumer, locals),
    }
}

/// The name, the proven initializer and the reference sites a producer
/// `let` statement carries, `None` when the statement does not prove one.
fn let_producer(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    local: &syn::Local,
    consumer: usize,
    locals: &mut usize,
) -> Option<(String, Lbl, Vec<Site>)> {
    let name = let_name(local)?;
    let init = local.init.as_ref()?;
    if init.diverge.is_some() {
        return None;
    }
    let (label, sites) = label_expr(renamer, env, &init.expr, consumer, locals);
    Some((name, label, sites))
}

/// The proven initializer type of `expr`, when it stays inside the total
/// primitive language.
fn total_init(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    expr: &syn::Expr,
) -> Option<PrimTy> {
    let mut locals = 0usize;
    let (label, _) = label_expr(renamer, env, expr, 0, &mut locals);
    label.prim()
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

/// A color in the refinement palette, the rank of a distinct signature.
type Color = u32;

/// The complete structural signature a node refines over, its node
/// references replaced by the previous round's colors.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Sig {
    /// The base operation, type and literal label, color-substituted.
    base: Lbl,
    /// The input neighbors' colors, in operand order.
    inputs: Vec<Color>,
    /// The full consumer descriptors, color-substituted, sorted.
    consumers: Vec<Lbl>,
    /// The previous round's class color, to avoid merging.
    prev_color: Color,
}

/// The color placeholder the signatures substitute node references with.
fn color_node(color: Color) -> Lbl {
    Lbl::Other("#".into(), vec![Lbl::Lit(color.to_string())])
}

/// Replace the node references of `lbl` with the referenced nodes'
/// previous-round colors.
fn color_substitute(lbl: &Lbl, colors: &[Color]) -> Lbl {
    match lbl {
        Lbl::Ref(value) => {
            if let Port::Node(index) = &value.port {
                color_node(colors[*index])
            } else {
                lbl.clone()
            }
        }
        Lbl::Op(op, left, right, prim) => Lbl::Op(
            *op,
            Box::new(color_substitute(left, colors)),
            Box::new(color_substitute(right, colors)),
            *prim,
        ),
        Lbl::Other(kind, children) => Lbl::Other(
            kind.clone(),
            children
                .iter()
                .map(|child| color_substitute(child, colors))
                .collect(),
        ),
        Lbl::Graph(root, definitions) => Lbl::Graph(
            Box::new(color_substitute(root, colors)),
            definitions
                .iter()
                .map(|definition| color_substitute(definition, colors))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The palette assignment: the colors the distinct signatures sort to.
fn assign_palette(sigs: &[Sig]) -> Vec<Color> {
    let mut order: Vec<usize> = (0..sigs.len()).collect();
    order.sort_by(|&a, &b| sigs[a].cmp(&sigs[b]));
    let mut colors = vec![0u32; sigs.len()];
    let mut palette = 0u32;
    let mut previous: Option<usize> = None;
    for &slot in &order {
        if previous.is_none_or(|prev| sigs[prev] != sigs[slot]) {
            palette += 1;
        }
        colors[slot] = palette;
        previous = Some(slot);
    }
    colors
}

/// Refine the region's colors until their class pattern stops refining or
/// a size guard stops them. Per-state work, never a search state.
fn refine_to_point(
    inits: &[Lbl],
    nodes: &[usize],
    deps: &[Vec<usize>],
    uses: &[Use],
    n: usize,
) -> Vec<Color> {
    let mut colors = vec![0u32; n];
    let mut passes = 0;
    loop {
        let previous = colors.clone();
        let mut sigs = Vec::with_capacity(n);
        for slot in 0..n {
            let mut consumers: Vec<Lbl> = uses
                .iter()
                .filter(|use_| use_.node == nodes[slot])
                .map(|use_| {
                    Lbl::Other(
                        "k".into(),
                        vec![
                            Lbl::Lit(use_.kind.to_string()),
                            Lbl::Lit(use_.index.to_string()),
                            color_substitute(&use_.label, &previous),
                        ],
                    )
                })
                .collect();
            consumers.sort();
            sigs.push(Sig {
                base: color_substitute(&inits[slot], &previous),
                inputs: deps[slot].iter().map(|&dep| previous[dep]).collect(),
                consumers,
                prev_color: previous[slot],
            });
        }
        let next = assign_palette(&sigs);
        let stable = next == colors;
        colors = next;
        passes += 1;
        if stable || passes > n {
            break;
        }
    }
    colors
}

/// Project cross-region references without expanding shared producers.
fn region_label(
    lbl: &Lbl,
    slots: &BTreeMap<usize, usize>,
    producers: &[Producer],
    sealed: &[Option<(usize, usize)>],
) -> Option<Lbl> {
    let mut projection = RegionProjection {
        slots,
        sealed,
        future: BTreeMap::new(),
        queue: Vec::new(),
    };
    let root = projection.label(lbl)?;
    let mut definitions = Vec::new();
    let mut next = 0;
    while next < projection.queue.len() {
        let producer = &producers[projection.queue[next]];
        definitions.push(projection.label(&producer.init)?);
        next += 1;
    }
    Some(if definitions.is_empty() {
        root
    } else {
        Lbl::Graph(Box::new(root), definitions)
    })
}

struct RegionProjection<'a> {
    slots: &'a BTreeMap<usize, usize>,
    sealed: &'a [Option<(usize, usize)>],
    future: BTreeMap<usize, usize>,
    queue: Vec<usize>,
}

impl RegionProjection<'_> {
    fn reference(&mut self, value: &Value, id: usize) -> Option<Lbl> {
        if let Some(&slot) = self.slots.get(&id) {
            return Some(Lbl::Ref(Value {
                port: Port::Node(slot),
                prim: value.prim,
            }));
        }
        if let Some((region, slot)) = self.sealed[id] {
            return Some(Lbl::Ref(Value {
                port: Port::Sealed(region, slot),
                prim: value.prim,
            }));
        }
        let index = if let Some(&index) = self.future.get(&id) {
            index
        } else {
            if self.slots.len() + self.queue.len() >= MAX_REGION_NODES {
                return None;
            }
            let index = self.queue.len();
            self.future.insert(id, index);
            self.queue.push(id);
            index
        };
        Some(Lbl::Future(index, value.prim))
    }

    fn label(&mut self, lbl: &Lbl) -> Option<Lbl> {
        Some(match lbl {
            Lbl::Ref(value) => match value.port {
                Port::Node(id) => return self.reference(value, id),
                _ => Lbl::Ref(value.clone()),
            },
            Lbl::Op(op, left, right, prim) => Lbl::Op(
                *op,
                Box::new(self.label(left)?),
                Box::new(self.label(right)?),
                *prim,
            ),
            Lbl::Other(kind, children) => Lbl::Other(
                kind.clone(),
                children
                    .iter()
                    .map(|child| self.label(child))
                    .collect::<Option<Vec<_>>>()?,
            ),
            other => other.clone(),
        })
    }
}

/// The first tie class whose interchange is not proven by the exact
/// incidence: members sharing a color but with unequal initializers,
/// dependencies, readers or consumer uses. Ties of proven direct twins,
/// full adjacency equality with multiplicity retained, are pruned.
fn find_branchable_tie(
    colors: &[Color],
    inits: &[Lbl],
    deps: &[Vec<usize>],
    uses: &[Use],
    nodes: &[usize],
) -> Option<Vec<usize>> {
    let mut groups: BTreeMap<Color, Vec<usize>> = BTreeMap::new();
    for (slot, &color) in colors.iter().enumerate() {
        groups.entry(color).or_default().push(slot);
    }
    for members in groups.into_values() {
        if members.len() < 2 {
            continue;
        }
        let twins = members.iter().all(|&slot| {
            inits[slot] == inits[members[0]]
                && deps[slot] == deps[members[0]]
                && !uses.iter().any(|use_| use_.node == nodes[slot])
                && !deps.iter().any(|readers| readers.contains(&slot))
        });
        if !twins {
            return Some(members);
        }
    }
    None
}

/// The region's canonical order: refine, branch on each member of the
/// first unproven tie in a bounded DFS, and take the smallest complete
/// labeled candidate. `None` when the expanded-state budget is crossed;
/// the whole region stays opaque.
struct Candidate {
    declarations: Vec<Lbl>,
    consumers: Vec<(u8, usize, Color, Lbl)>,
    order: Vec<usize>,
}

fn complete_candidate(
    inits: &[Lbl],
    nodes: &[usize],
    uses: &[Use],
    order: Vec<usize>,
) -> Candidate {
    let mut positions = vec![0; order.len()];
    for (position, &slot) in order.iter().enumerate() {
        positions[slot] = Color::try_from(position).expect("a bounded region position fits u32");
    }
    let declarations = order
        .iter()
        .map(|&slot| color_substitute(&inits[slot], &positions))
        .collect();
    let slots: BTreeMap<_, _> = nodes
        .iter()
        .copied()
        .enumerate()
        .map(|(slot, id)| (id, slot))
        .collect();
    let mut consumers: Vec<_> = uses
        .iter()
        .map(|use_| {
            (
                use_.kind,
                use_.index,
                positions[slots[&use_.node]],
                color_substitute(&use_.label, &positions),
            )
        })
        .collect();
    consumers.sort();
    Candidate {
        declarations,
        consumers,
        order,
    }
}

fn label_region_search(
    base: &[Lbl],
    inits: &[Lbl],
    nodes: &[usize],
    deps: &[Vec<usize>],
    uses: &[Use],
    states: &mut usize,
    depth: usize,
) -> Option<Candidate> {
    *states += 1;
    if *states > MAX_LABEL_STATES {
        return None;
    }
    let colors = refine_to_point(inits, nodes, deps, uses, nodes.len());
    if let Some(members) = find_branchable_tie(&colors, inits, deps, uses, nodes) {
        let mut best: Option<Candidate> = None;
        for member in members {
            let mut marked = inits.to_vec();
            marked[member] = Lbl::Other(
                "I".into(),
                vec![Lbl::Lit(depth.to_string()), base[member].clone()],
            );
            let candidate =
                label_region_search(base, &marked, nodes, deps, uses, states, depth + 1)?;
            if best.as_ref().is_none_or(|winner| {
                (&candidate.declarations, &candidate.consumers)
                    < (&winner.declarations, &winner.consumers)
            }) {
                best = Some(candidate);
            }
        }
        return best;
    }
    let order = kahn_order(&colors, deps, nodes.len())?;
    Some(complete_candidate(base, nodes, uses, order))
}

/// Emit dependency-ready nodes in exact color order.
fn kahn_order(colors: &[Color], deps: &[Vec<usize>], n: usize) -> Option<Vec<usize>> {
    let mut pending: Vec<_> = deps.iter().map(Vec::len).collect();
    let mut readers = vec![Vec::new(); n];
    let mut ready = alloc::collections::BTreeSet::new();
    for (slot, inputs) in deps.iter().enumerate() {
        for &input in inputs {
            readers[input].push(slot);
        }
        if inputs.is_empty() {
            ready.insert((colors[slot], slot));
        }
    }
    let mut order = Vec::with_capacity(n);
    while let Some((_, slot)) = ready.pop_first() {
        order.push(slot);
        for &reader in &readers[slot] {
            pending[reader] -= 1;
            if pending[reader] == 0 {
                ready.insert((colors[reader], reader));
            }
        }
    }
    (order.len() == n).then_some(order)
}

/// Label one region's producers from their dependency edges and their
/// consumer uses, returning the canonical order of the region's slots or
/// `None` when a bound is crossed.
fn schedule_region(
    region: &Region,
    producers: &[Producer],
    uses: &[Use],
    sealed: &[Option<(usize, usize)>],
) -> Option<Vec<usize>> {
    let nodes = &region.producers;
    let n = nodes.len();
    let mut slots: BTreeMap<usize, usize> = BTreeMap::new();
    for (slot, &id) in nodes.iter().enumerate() {
        slots.insert(id, slot);
    }
    if nodes
        .iter()
        .enumerate()
        .skip(1)
        .all(|(slot, &id)| producers[id].deps.contains(&(slot - 1)))
    {
        // Every predecessor is required, so only source order is legal.
        for use_ in uses.iter().filter(|use_| slots.contains_key(&use_.node)) {
            region_label(&use_.label, &slots, producers, sealed)?;
        }
        return Some((0..n).collect());
    }
    let inits: Vec<Lbl> = nodes
        .iter()
        .map(|&id| region_label(&producers[id].init, &slots, producers, sealed))
        .collect::<Option<_>>()?;
    let deps: Vec<Vec<usize>> = nodes.iter().map(|&id| producers[id].deps.clone()).collect();
    let region_uses: Vec<Use> = uses
        .iter()
        .filter(|use_| slots.contains_key(&use_.node))
        .map(|use_| {
            Some(Use {
                node: use_.node,
                kind: use_.kind,
                index: use_.index,
                label: region_label(&use_.label, &slots, producers, sealed)?,
            })
        })
        .collect::<Option<_>>()?;
    let mut states = 0usize;
    label_region_search(&inits, &inits, nodes, &deps, &region_uses, &mut states, 0)
        .map(|candidate| candidate.order)
}

/// Record the producer uses the sites carry on `kind` with `index`, and
/// dead-mark the region of a producer whose storage the sites observe.
fn note_sites(
    sites: &[Site],
    uses: &mut Vec<Use>,
    kind: u8,
    index: usize,
    label: &Lbl,
    regions: &mut [Region],
    producers: &[Producer],
) {
    for site in sites {
        if let Port::Node(node) = &site.port {
            uses.push(Use {
                node: *node,
                kind,
                index,
                label: label.clone(),
            });
            if site.ctx == Ctx::Storage {
                regions[producers[*node].region].dead = true;
            }
        }
    }
}

fn record_all_opaque(renamer: &mut Renamer, block: &mut syn::Block) {
    renamer.facts.blocks.insert(
        core::ptr::from_ref(block),
        BlockProof {
            statements: vec![OPAQUE; block.stmts.len()],
            types: vec![None; block.stmts.len()],
            tail_types: Vec::new(),
        },
    );
    renamer.push_facts(BlockFacts {
        plans: BTreeMap::new(),
        permutation: Vec::new(),
    });
}

struct BlockScan {
    env: BTreeMap<String, Value>,
    producers: Vec<Producer>,
    regions: Vec<Region>,
    uses: Vec<Use>,
    tags: Vec<u8>,
    tail_types: Vec<Option<PrimTy>>,
    locals: usize,
    unreachable: bool,
    tail_pending: bool,
    open: Option<usize>,
}

impl BlockScan {
    fn new(env: BTreeMap<String, Value>, locals: usize, count: usize) -> Self {
        Self {
            env,
            producers: Vec::new(),
            regions: Vec::new(),
            uses: Vec::new(),
            tags: vec![OPAQUE; count],
            tail_types: Vec::new(),
            locals,
            unreachable: false,
            tail_pending: false,
            open: None,
        }
    }

    fn note(&mut self, sites: &[Site], kind: u8, index: usize, label: &Lbl) {
        note_sites(
            sites,
            &mut self.uses,
            kind,
            index,
            label,
            &mut self.regions,
            &self.producers,
        );
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
            let (label, sites) = statement_label(renamer, &self.env, stmt, index, &mut self.locals);
            self.note(&sites, USE_UNREACHABLE, index, &label);
            return;
        }
        if let syn::Stmt::Local(local) = stmt {
            self.local(renamer, index, stmt, local);
        } else {
            self.other(renamer, index, stmt);
        }
    }

    fn local(&mut self, renamer: &Renamer, index: usize, stmt: &syn::Stmt, local: &syn::Local) {
        if let Some((name, init, sites)) =
            let_producer(renamer, &self.env, local, index, &mut self.locals)
            && init.prim().is_some()
        {
            self.producer(index, name, init, &sites);
            return;
        }
        let (label, sites) = statement_label(renamer, &self.env, stmt, index, &mut self.locals);
        self.note(&sites, USE_OPAQUE, index, &label);
        if let Some(name) = local_binding_name(local) {
            self.invalidate(name);
        }
        self.open = None;
    }

    fn producer(&mut self, index: usize, name: String, init: Lbl, sites: &[Site]) {
        let region = *self.open.get_or_insert_with(|| {
            self.regions.push(Region {
                producers: Vec::new(),
                start: index,
                dead: false,
            });
            self.regions.len() - 1
        });
        if self.regions[region].producers.len() == MAX_REGION_NODES {
            self.regions[region].dead = true;
            self.env.insert(
                name,
                Value {
                    port: Port::Local(self.locals),
                    prim: init.prim(),
                },
            );
            self.locals += 1;
            return;
        }
        let slot = self.regions[region].producers.len();
        let mut deps = Vec::new();
        for site in sites {
            if let Port::Node(id) = &site.port {
                let producer = &self.producers[*id];
                if producer.region == region {
                    deps.push(producer.slot);
                } else {
                    self.uses.push(Use {
                        node: *id,
                        kind: USE_DOWN,
                        index: 0,
                        label: init.clone(),
                    });
                }
                if site.ctx == Ctx::Storage {
                    self.regions[producer.region].dead = true;
                }
            }
        }
        let prim = init.prim().expect("a proven initializer");
        self.producers.push(Producer {
            stmt: index,
            region,
            slot,
            name,
            init,
            prim,
            deps,
        });
        let id = self.producers.len() - 1;
        self.regions[region].producers.push(id);
        self.env.insert(
            self.producers[id].name.clone(),
            Value {
                port: Port::Node(id),
                prim: Some(prim),
            },
        );
    }

    fn other(&mut self, renamer: &Renamer, index: usize, stmt: &syn::Stmt) {
        let kind = stmt_kind(renamer, stmt);
        let (label, sites) = statement_label(renamer, &self.env, stmt, index, &mut self.locals);
        match kind {
            StmtKind::Barrier => {
                self.tags[index] = OBSERVATION;
                self.captures(index, stmt, &label);
                self.note(&sites, USE_OBSERVED, index, &label);
            }
            StmtKind::Boundary(definite) => {
                self.tags[index] = BOUNDARY;
                self.note(&sites, USE_BOUNDARY, index, &label);
                self.open = None;
                self.unreachable = definite;
                return;
            }
            StmtKind::Plain => self.note(&sites, USE_OPAQUE, index, &label),
        }
        if let Some(target) = assign_target(stmt) {
            if let Some(value) = self.env.get(&target)
                && let Port::Node(id) = &value.port
            {
                self.regions[self.producers[*id].region].dead = true;
            }
            self.invalidate(target);
        }
        self.open = None;
    }

    fn invalidate(&mut self, name: String) {
        self.env.insert(
            name,
            Value {
                port: Port::Local(self.locals),
                prim: None,
            },
        );
        self.locals += 1;
    }

    fn captures(&mut self, index: usize, stmt: &syn::Stmt, label: &Lbl) {
        let mac = stmt_macro(stmt).expect("barrier statements name a macro");
        let (format_at, _) =
            crate::drift::format_operand(&mac.path).expect("barrier macros are format macros");
        let args = macro_args(mac);
        let Some(syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(lit),
            ..
        })) = args.get(format_at)
        else {
            return;
        };
        for (position, capture) in format_refs(&lit.value(), &args).into_iter().enumerate() {
            if let Some(value) = self.env.get(&capture)
                && let Port::Node(node) = &value.port
            {
                self.uses.push(Use {
                    node: *node,
                    kind: USE_CAPTURE,
                    index,
                    label: Lbl::Other(
                        "capture".into(),
                        vec![Lbl::Int(position as u128, None), label.clone()],
                    ),
                });
            }
        }
    }

    fn finish_tail(&mut self, renamer: &Renamer, block: &syn::Block) {
        if !self.tail_pending {
            return;
        }
        let Some(syn::Stmt::Expr(expr, None)) = block.stmts.last() else {
            unreachable!("a pending tail is an expression");
        };
        *self
            .tags
            .last_mut()
            .expect("a pending tail has a statement role") =
            if self.unreachable { UNREACHABLE } else { TAIL };
        if self.unreachable {
            let (label, sites) = label_expr(renamer, &self.env, expr, 0, &mut self.locals);
            self.note(&sites, USE_UNREACHABLE, 0, &label);
            return;
        }
        if let syn::Expr::Tuple(tuple) = peel_paren(expr) {
            for (position, element) in tuple.elems.iter().enumerate() {
                self.tail_element(renamer, element, position);
            }
        } else {
            self.tail_element(renamer, expr, 0);
        }
    }

    fn tail_element(&mut self, renamer: &Renamer, expr: &syn::Expr, position: usize) {
        let (label, sites) = label_expr(renamer, &self.env, expr, position, &mut self.locals);
        self.note(&sites, USE_TAIL, position, &label);
        self.tail_types.push(label.prim());
    }

    fn schedule(&mut self, region_seq: &[usize]) -> Vec<Vec<usize>> {
        let mut orders = Vec::with_capacity(self.regions.len());
        let mut sealed = vec![None; self.producers.len()];
        for (region_index, region) in self.regions.iter_mut().enumerate() {
            let node_count = region.producers.len();
            let order = if region.dead || node_count < 2 {
                (0..node_count).collect()
            } else if let Some(order) =
                schedule_region(region, &self.producers, &self.uses, &sealed)
            {
                order
            } else {
                region.dead = true;
                (0..node_count).collect()
            };
            for (position, &slot) in order.iter().enumerate() {
                sealed[region.producers[slot]] = Some((region_seq[region_index], position));
            }
            orders.push(order);
        }
        orders
    }

    fn permutation(&self, orders: &[Vec<usize>], count: usize) -> Vec<usize> {
        let mut permutation: Vec<usize> = (0..count).collect();
        for (region_index, region) in self.regions.iter().enumerate() {
            if !region.dead {
                for (position, &slot) in orders[region_index].iter().enumerate() {
                    permutation[self.producers[region.producers[slot]].stmt] =
                        region.start + position;
                }
            }
        }
        permutation
    }

    fn binding_plans(
        &self,
        block: &syn::Block,
        permutation: &[usize],
        region_seq: &[usize],
    ) -> BTreeMap<*const syn::Pat, BindingPlan> {
        let mut plans = BTreeMap::new();
        for producer in &self.producers {
            let syn::Stmt::Local(local) = &block.stmts[producer.stmt] else {
                unreachable!("a producer is a local declaration");
            };
            let source = isize::try_from(producer.stmt).expect("a statement index fits isize");
            let destination =
                isize::try_from(permutation[producer.stmt]).expect("a statement index fits isize");
            plans.insert(
                core::ptr::from_ref(&local.pat),
                BindingPlan {
                    offset: destination - source,
                    prim: Some(producer.prim),
                    port: Port::Sealed(
                        region_seq[producer.region],
                        permutation[producer.stmt] - self.regions[producer.region].start,
                    ),
                },
            );
        }
        plans
    }
}

/// Resolve original statements and plan delayed region permutations.
pub(crate) fn analyze(renamer: &mut Renamer, block: &mut syn::Block) {
    let (frozen, mut seq, locals) = {
        let scope = renamer.scopes.last_mut().expect("a fn scope");
        (scope.frozen, scope.seq, scope.next_local)
    };
    if frozen || renamer.depth() > MAX_BLOCK_DEPTH {
        record_all_opaque(renamer, block);
        return;
    }
    let mut env = BTreeMap::new();
    renamer.frame_env(&mut env);
    let mut scan = BlockScan::new(env, locals, block.stmts.len());
    for (index, stmt) in block.stmts.iter().enumerate() {
        scan.statement(renamer, index, stmt, index + 1 == block.stmts.len());
    }
    scan.finish_tail(renamer, block);
    let region_seq: Vec<usize> = (0..scan.regions.len())
        .map(|_| {
            let current = seq;
            seq += 1;
            current
        })
        .collect();
    let orders = scan.schedule(&region_seq);
    let permutation = scan.permutation(&orders, block.stmts.len());
    let plans = scan.binding_plans(block, &permutation, &region_seq);
    let mut types = vec![None; block.stmts.len()];
    for producer in &scan.producers {
        let dead = scan.regions[producer.region].dead;
        scan.tags[producer.stmt] = if dead { OPAQUE } else { PRODUCER };
        if !dead {
            types[producer.stmt] = Some(producer.prim);
        }
    }
    let scope = renamer.scopes.last_mut().expect("a fn scope");
    scope.seq = seq;
    scope.next_local = scan.locals;
    renamer.facts.blocks.insert(
        core::ptr::from_ref(block),
        BlockProof {
            statements: scan.tags,
            types,
            tail_types: scan.tail_types,
        },
    );
    renamer.push_facts(BlockFacts { plans, permutation });
}
