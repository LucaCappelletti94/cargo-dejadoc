//! Exact, bounded scheduling of proven scalar declaration regions.

use core::cell::RefCell;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use quote::ToTokens as _;
use syn::visit::Visit;

use crate::alpha::{MacroArgs, Renamer};
use crate::reference::{BlockProof, ReferenceFacts};
use crate::scope::{FreeKey, Ns, PrimTy, unraw};

mod values;

/// The graph nodes a region may still be labeled with.
pub(crate) const MAX_REGION_NODES: usize = 16_384;
/// The block nesting level a function scope may analyze.
pub(crate) const MAX_BLOCK_DEPTH: usize = 64;
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

/// A reference and its separate helper-cache handle.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct Value {
    pub(crate) port: Port,
    pub(crate) prim: Option<PrimTy>,
    pub(crate) helper: Option<crate::scope::HelperId>,
}

/// A labeled node of the expression language the analysis reads.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Lbl {
    /// An integer literal, `None` when the suffix leaves the type unproven.
    Int(u128, Option<PrimTy>),
    /// A boolean literal.
    Bool(bool),
    /// A proven primitive boolean negation of a total boolean scalar.
    Not(Box<Lbl>),
    /// Any other literal, by its text.
    Lit(String),
    /// A value reference.
    Ref(Value),
    /// A total bitwise or valid shift over proven scalars.
    Op(char, Box<Lbl>, Box<Lbl>, PrimTy),
    /// A flattened bitwise chain: one operator and type, sorted leaves,
    /// multiplicity retained, literal leaves folded.
    Flat(char, Vec<Lbl>, PrimTy),
    /// Any other expression, by a kind tag and its ordered children.
    Other(String, Vec<Lbl>),
    /// A first-reference index in a projected future-producer graph.
    Future(usize, Option<PrimTy>),
    /// An ordered root and its sharing-preserving future definitions.
    Graph(Box<Lbl>, Vec<Lbl>),
    /// A total call of an accepted helper: callee identity, ordered
    /// argument labels, and the helper's result type.
    Call(Box<Lbl>, Vec<Lbl>, PrimTy),
}

/// The recorded field orders of an analyzed block, keyed by the first owned field buffer, None marking a rejected group.
pub(crate) type FieldOrders = BTreeMap<*const syn::FieldValue, Option<Vec<usize>>>;

pub(crate) enum FieldOrder<'a> {
    Rejected,
    Admitted(&'a [usize]),
}

impl Lbl {
    /// The proven scalar type of a total expression, `None` otherwise.
    pub(crate) fn prim(&self) -> Option<PrimTy> {
        match self {
            Self::Int(_, prim) => *prim,
            Self::Bool(_) | Self::Not(_) => Some(PrimTy::Bool),
            Self::Ref(value) => value.prim,
            Self::Op(_, _, _, prim) | Self::Flat(_, _, prim) | Self::Call(_, _, prim) => {
                Some(*prim)
            }
            _ => None,
        }
    }

    pub(crate) fn literal_bits(&self) -> Option<u128> {
        match self {
            Self::Int(bits, _) => Some(*bits),
            Self::Bool(value) => Some(u128::from(*value)),
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
pub(crate) struct Site {
    port: Port,
    ctx: Ctx,
}

/// Storage identity, excluded from structural comparisons.
#[derive(Clone, Copy)]
struct ConsumerId(usize);

struct Consumer {
    kind: u8,
    index: usize,
    label: Lbl,
}

/// One reference site with its complete consumer descriptor.
struct Use {
    node: usize,
    consumer: ConsumerId,
}

/// Immutable consumer labels with multiplicity-preserving producer uses.
#[derive(Default)]
struct Uses {
    consumers: Vec<Consumer>,
    entries: Vec<Use>,
}

impl Uses {
    fn insert(&mut self, kind: u8, index: usize, label: Lbl) -> ConsumerId {
        let id = ConsumerId(self.consumers.len());
        self.consumers.push(Consumer { kind, index, label });
        id
    }

    fn note(
        &mut self,
        sites: &[Site],
        kind: u8,
        index: usize,
        label: Lbl,
        regions: &mut [Region],
        producers: &[Producer],
    ) {
        if !sites.iter().any(|site| matches!(site.port, Port::Node(_))) {
            return;
        }
        let consumer = self.insert(kind, index, label);
        for site in sites {
            if let Port::Node(node) = &site.port {
                self.entries.push(Use {
                    node: *node,
                    consumer,
                });
                if site.ctx == Ctx::Storage {
                    regions[producers[*node].region].dead = true;
                }
            }
        }
    }
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
    /// A function-level `cfg` or `cfg_attr` the source cannot settle.
    pub(crate) cfg: bool,
    pub(crate) observed: BTreeSet<Port>,
}

pub(crate) struct BindingPlan {
    pub(crate) offset: isize,
    pub(crate) prim: Option<PrimTy>,
    pub(crate) port: Port,
    pub(crate) active: bool,
}

pub(crate) struct BlockFacts {
    plans: BTreeMap<*const syn::Pat, BindingPlan>,
    permutation: Vec<usize>,
    field_orders: FieldOrders,
    original: Option<syn::Block>,
}

impl BlockFacts {
    pub(crate) fn plan(&self, pat: &syn::Pat) -> Option<&BindingPlan> {
        self.plans.get(&core::ptr::from_ref(pat))
    }

    pub(crate) fn can_rewrite(&self) -> bool {
        !self.permutation.is_empty()
    }
    pub(crate) fn field_order(&self, first: &syn::FieldValue) -> Option<FieldOrder<'_>> {
        self.field_orders
            .get(&core::ptr::from_ref(first))
            .map(|record| {
                record
                    .as_deref()
                    .map_or(FieldOrder::Rejected, FieldOrder::Admitted)
            })
    }

    pub(crate) fn original_on_exhaustion(
        &mut self,
        block: &syn::Block,
        outer: usize,
    ) -> Option<syn::Block> {
        let original = self.original.take()?;
        if values::within_bounds(block, outer) {
            None
        } else {
            Some(original)
        }
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
                _ if format_name_char(c) => name.push(c),
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
                .find(|(_, c)| !format_name_char(*c))
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

fn format_name_char(c: char) -> bool {
    !c.is_whitespace() && (!c.is_ascii_punctuation() || c == '_')
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

/// Whether `op` is a compound assignment.
pub(crate) fn is_assign_op(op: syn::BinOp) -> bool {
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
fn op_text(op: syn::BinOp) -> String {
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

/// An associative bitwise operation over proven primitives.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BitwiseOp {
    And,
    Or,
    Xor,
}

impl BitwiseOp {
    pub(crate) fn from_bin_op(op: syn::BinOp) -> Option<Self> {
        match op {
            syn::BinOp::BitAnd(_) => Some(Self::And),
            syn::BinOp::BitOr(_) => Some(Self::Or),
            syn::BinOp::BitXor(_) => Some(Self::Xor),
            _ => None,
        }
    }

    pub(crate) fn tag(self) -> char {
        match self {
            Self::And => '&',
            Self::Or => '|',
            Self::Xor => '^',
        }
    }

    pub(crate) fn fold(self, left: u128, right: u128) -> u128 {
        match self {
            Self::And => left & right,
            Self::Or => left | right,
            Self::Xor => left ^ right,
        }
    }
}

/// The comparison emitted after exchanging ordered operands.
#[derive(Clone, Copy)]
pub(crate) enum ReversedComparison {
    Lt,
    Le,
}

impl ReversedComparison {
    pub(crate) fn bin_op(self) -> syn::BinOp {
        match self {
            Self::Lt => syn::BinOp::Lt(syn::token::Lt::default()),
            Self::Le => syn::BinOp::Le(syn::token::Le::default()),
        }
    }
}

/// The total form of a binary operation over proven scalars.
pub(crate) enum TotalBinary {
    /// Kept in operand order, a valid shift or an oriented comparison.
    Ordered {
        tag: char,
        reverse: Option<ReversedComparison>,
        prim: PrimTy,
    },
    /// An unordered bitwise chain of `&`, `|` or `^`.
    Flat(BitwiseOp, PrimTy),
}

pub(crate) fn total_binary(bin_op: syn::BinOp, left: &Lbl, right: &Lbl) -> Option<TotalBinary> {
    if let Some(op) = BitwiseOp::from_bin_op(bin_op) {
        return Some(TotalBinary::Flat(op, scalar_prim(left, right)?));
    }
    match bin_op {
        syn::BinOp::Shl(_) | syn::BinOp::Shr(_) => {
            let prim = left.prim()?;
            let PrimTy::Int { width, .. } = prim else {
                return None;
            };
            match right {
                Lbl::Int(count, _) if *count < u128::from(width.bits()) => {
                    Some(TotalBinary::Ordered {
                        tag: match bin_op {
                            syn::BinOp::Shl(_) => '«',
                            _ => '»',
                        },
                        reverse: None,
                        prim,
                    })
                }
                _ => None,
            }
        }
        _ => comparison(bin_op, left, right),
    }
}

/// The scalar type both sides prove, an unsuffixed integer literal
/// adopting the established type.
fn scalar_prim(left: &Lbl, right: &Lbl) -> Option<PrimTy> {
    match (left.prim(), right.prim()) {
        (Some(left), Some(right)) if left == right => Some(left),
        (Some(left @ PrimTy::Int { .. }), None) if matches!(right, Lbl::Int(_, None)) => Some(left),
        (None, Some(right @ PrimTy::Int { .. })) if matches!(left, Lbl::Int(_, None)) => {
            Some(right)
        }
        _ => None,
    }
}

/// A comparison or a logical operation over proven scalars, oriented by
/// kind, a bool result.
fn comparison(bin_op: syn::BinOp, left: &Lbl, right: &Lbl) -> Option<TotalBinary> {
    let prim = scalar_prim(left, right)?;
    let (tag, reverse) = match bin_op {
        syn::BinOp::Eq(_) => ('=', None),
        syn::BinOp::Ne(_) => ('!', None),
        syn::BinOp::Lt(_) => ('<', None),
        syn::BinOp::Le(_) => ('≤', None),
        syn::BinOp::Gt(_) => ('<', Some(ReversedComparison::Lt)),
        syn::BinOp::Ge(_) => ('≤', Some(ReversedComparison::Le)),
        syn::BinOp::And(_) if prim == PrimTy::Bool => ('∧', None),
        syn::BinOp::Or(_) if prim == PrimTy::Bool => ('∨', None),
        _ => return None,
    };
    Some(TotalBinary::Ordered {
        tag,
        reverse,
        prim: PrimTy::Bool,
    })
}

/// Negate a total boolean label without changing logical operand order.
pub(crate) fn not_label(operand: Lbl) -> Lbl {
    match operand {
        Lbl::Not(inner) => *inner,
        Lbl::Bool(value) => Lbl::Bool(!value),
        Lbl::Op(tag @ ('∧' | '∨'), mut left, mut right, _) => {
            *left = not_label(*left);
            *right = not_label(*right);
            Lbl::Op(
                if tag == '∧' { '∨' } else { '∧' },
                left,
                right,
                PrimTy::Bool,
            )
        }
        other => Lbl::Not(Box::new(other)),
    }
}

/// The total boolean operand compared with literal `true`.
pub(crate) fn true_comparison(bin_op: syn::BinOp, left: &Lbl, right: &Lbl) -> Option<Lbl> {
    if !matches!(bin_op, syn::BinOp::Eq(_)) {
        return None;
    }
    match (left, right) {
        (other, Lbl::Bool(true)) | (Lbl::Bool(true), other)
            if other.prim() == Some(PrimTy::Bool) =>
        {
            Some(other.clone())
        }
        _ => None,
    }
}

/// The operands of a flat chain, flattening chains of the same operator
/// and type.
pub(crate) fn collect_leaves(op: BitwiseOp, prim: PrimTy, lbl: Lbl, out: &mut Vec<Lbl>) {
    match lbl {
        Lbl::Flat(inner_op, leaves, inner_prim) if inner_op == op.tag() && inner_prim == prim => {
            out.extend(leaves);
        }
        other => out.push(other),
    }
}

/// A flat chain's canonical label, sorted leaves with the literal
/// subset folded, a lone literal standing for the whole chain.
pub(crate) fn flat_label(op: BitwiseOp, prim: PrimTy, mut leaves: Vec<Lbl>) -> Lbl {
    leaves.sort();
    if op != BitwiseOp::Xor {
        leaves.dedup();
    }
    fold_literal_leaves(op, prim, &mut leaves);
    match leaves.as_slice() {
        [only] => only.clone(),
        _ => Lbl::Flat(op.tag(), leaves, prim),
    }
}

/// The chain's literal leaves folded into one, re-sorted into place.
fn fold_literal_leaves(op: BitwiseOp, prim: PrimTy, leaves: &mut Vec<Lbl>) {
    let is_literal = |leaf: &Lbl| leaf.literal_bits().is_some();
    if leaves.iter().filter(|leaf| is_literal(leaf)).count() < 2 {
        return;
    }
    let mut acc = None;
    leaves.retain(|leaf| {
        let Some(bits) = leaf.literal_bits() else {
            return true;
        };
        acc = Some(match acc {
            None => bits,
            Some(left) => op.fold(left, bits),
        });
        false
    });
    leaves.push(literal_label(
        prim,
        acc.expect("at least two literal operands"),
    ));
    leaves.sort();
}

/// A folded literal under the chain's proven primitive type.
pub(crate) fn literal_label(prim: PrimTy, bits: u128) -> Lbl {
    match prim {
        PrimTy::Int { .. } => Lbl::Int(bits, Some(prim)),
        PrimTy::Bool => Lbl::Bool(bits != 0),
    }
}

/// Count source-label nodes before future projection.
pub(crate) fn label_nodes(lbl: &Lbl) -> usize {
    match lbl {
        Lbl::Not(inner) => 1 + label_nodes(inner),
        Lbl::Op(_, left, right, _) => 1 + label_nodes(left) + label_nodes(right),
        Lbl::Flat(_, leaves, _) | Lbl::Other(_, leaves) => {
            1 + leaves.iter().map(label_nodes).sum::<usize>()
        }
        Lbl::Call(callee, args, _) => {
            1 + label_nodes(callee) + args.iter().map(label_nodes).sum::<usize>()
        }
        _ => 1,
    }
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
pub(crate) fn pat_names(pat: &syn::Pat) -> Vec<String> {
    match pat {
        syn::Pat::Ident(ident) => {
            let text = ident.ident.to_string();
            let mut names = vec![unraw(&text).to_string()];
            if let Some((_, subpat)) = &ident.subpat {
                names.extend(pat_names(subpat));
            }
            names
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

/// The assigned binding's name, when the target is a clean identifier.
pub(crate) fn assign_target(stmt: &syn::Stmt) -> Option<String> {
    let syn::Stmt::Expr(expr, _) = stmt else {
        return None;
    };
    let assign: &syn::Expr = match peel_paren(expr) {
        syn::Expr::Assign(assign) => &assign.left,
        syn::Expr::Binary(binary) if is_assign_op(binary.op) => &binary.left,
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
pub(crate) enum StmtKind {
    /// A known observation with modeled value inputs.
    Barrier,
    /// A control boundary, a possible divergence.
    Boundary(bool),
    /// Everything else, opaque syntax.
    Plain,
}

pub(crate) fn stmt_kind(renamer: &Renamer, stmt: &syn::Stmt) -> StmtKind {
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

pub(crate) fn scalar_leaf(env: &BTreeMap<String, Value>, expr: &syn::Expr) -> Option<Lbl> {
    match expr {
        syn::Expr::Lit(lit) => Some(label_lit(&lit.lit)),
        syn::Expr::Unary(_) => {
            crate::constants::signed_literal(expr).map(|(prim, bits)| Lbl::Int(bits, Some(prim)))
        }
        syn::Expr::Path(path) if path.qself.is_none() && path.path.segments.len() == 1 => {
            let name = path.path.segments[0].ident.to_string();
            env.get(unraw(&name)).cloned().map(Lbl::Ref)
        }
        _ => None,
    }
}

/// The label and reference sites an expression carries. Nested blocks,
/// loops, arms and closure bodies extend the environment with their local
/// binders; borrows, addresses, receivers and captures mark the sites on
/// the storage of the referenced producers.
pub(crate) fn label_expr(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    expr: &syn::Expr,
    consumer: usize,
    locals: &mut usize,
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    match expr {
        syn::Expr::Lit(lit) => (label_lit(&lit.lit), Vec::new()),
        syn::Expr::Path(path) if path.qself.is_none() => label_path(env, &path.path),
        syn::Expr::Path(_) => (Lbl::Other("q".into(), Vec::new()), Vec::new()),
        syn::Expr::Binary(bin) => label_binary(renamer, env, bin, consumer, locals, orders),
        syn::Expr::Paren(paren) => label_expr(renamer, env, &paren.expr, consumer, locals, orders),
        syn::Expr::Group(group) => label_expr(renamer, env, &group.expr, consumer, locals, orders),
        syn::Expr::Tuple(tuple) => {
            label_tuple(renamer, env, &tuple.elems, consumer, locals, orders)
        }
        syn::Expr::Call(call) => label_call(renamer, env, call, consumer, locals, orders),
        syn::Expr::MethodCall(method) => {
            label_method(renamer, env, method, consumer, locals, orders)
        }
        syn::Expr::Macro(mac) => macro_label(renamer, env, &mac.mac, consumer, locals, orders),
        syn::Expr::Reference(reference) => {
            label_reference(renamer, env, reference, consumer, locals, orders)
        }
        syn::Expr::Unary(unary) => match crate::constants::signed_literal(expr) {
            Some((prim, bits)) => (Lbl::Int(bits, Some(prim)), Vec::new()),
            None => label_unary(renamer, env, unary, consumer, locals, orders),
        },
        syn::Expr::Block(block) => {
            label_block(renamer, env, &block.block, consumer, locals, orders)
        }
        syn::Expr::If(r#if) => label_if_expr(renamer, env, r#if, consumer, locals, orders),
        syn::Expr::ForLoop(for_loop) => {
            label_for_loop(renamer, env, for_loop, consumer, locals, orders)
        }
        syn::Expr::While(r#while) => label_while(renamer, env, r#while, consumer, locals, orders),
        syn::Expr::Loop(r#loop) => label_loop(renamer, env, r#loop, consumer, locals, orders),
        syn::Expr::Match(r#match) => {
            label_match_expr(renamer, env, r#match, consumer, locals, orders)
        }
        syn::Expr::Closure(closure) => {
            label_closure(renamer, env, closure, consumer, locals, orders)
        }
        syn::Expr::Array(array) => label_array(renamer, env, array, consumer, locals, orders),
        syn::Expr::Repeat(repeat) => label_repeat(renamer, env, repeat, consumer, locals, orders),
        syn::Expr::Range(range) => label_range(renamer, env, range, consumer, locals, orders),
        syn::Expr::Index(index_expr) => {
            label_index(renamer, env, index_expr, consumer, locals, orders)
        }
        syn::Expr::Field(field) => label_field(renamer, env, field, consumer, locals, orders),
        syn::Expr::Struct(struct_expr) => {
            label_struct_expr(renamer, env, struct_expr, consumer, locals, orders)
        }
        syn::Expr::Let(let_expr) => {
            label_let_expr(renamer, env, let_expr, consumer, locals, orders)
        }
        syn::Expr::Assign(assign) => label_assign(renamer, env, assign, consumer, locals, orders),
        syn::Expr::Return(r#return) => label_optional_value(
            "ret",
            renamer,
            env,
            r#return.expr.as_deref(),
            consumer,
            locals,
            orders,
        ),
        syn::Expr::Break(break_expr) => label_optional_value(
            "br",
            renamer,
            env,
            break_expr.expr.as_deref(),
            consumer,
            locals,
            orders,
        ),
        syn::Expr::Try(try_expr) => label_try(renamer, env, try_expr, consumer, locals, orders),
        syn::Expr::TryBlock(block) => {
            label_blk(renamer, env, &block.block, consumer, locals, orders)
        }
        syn::Expr::Unsafe(block) => label_blk(renamer, env, &block.block, consumer, locals, orders),
        syn::Expr::Async(block) => label_blk(renamer, env, &block.block, consumer, locals, orders),
        syn::Expr::Const(block) => label_blk(renamer, env, &block.block, consumer, locals, orders),
        syn::Expr::Verbatim(tokens) => (
            Lbl::Other("v".into(), vec![Lbl::Lit(tokens.to_string())]),
            Vec::new(),
        ),
        syn::Expr::Await(await_expr) => {
            label_await(renamer, env, await_expr, consumer, locals, orders)
        }
        syn::Expr::Cast(cast) => label_cast(renamer, env, cast, consumer, locals, orders),
        syn::Expr::RawAddr(raw) => label_raw_addr(renamer, env, raw, consumer, locals, orders),
        _ => (Lbl::Other("e".into(), Vec::new()), Vec::new()),
    }
}

/// The label and sites of a path, a single-segment unqualified name
/// resolved through the environment.
fn label_path(env: &BTreeMap<String, Value>, path: &syn::Path) -> (Lbl, Vec<Site>) {
    if path.segments.len() == 1 {
        let name = path.segments[0].ident.to_string();
        return match env.get(unraw(&name)) {
            Some(value) => (
                Lbl::Ref(Value {
                    helper: None,
                    ..value.clone()
                }),
                vec![Site {
                    port: value.port.clone(),
                    ctx: Ctx::Value,
                }],
            ),
            None => (
                Lbl::Ref(Value {
                    port: Port::Free(FreeKey::Unresolved(unraw(&name).to_string())),
                    prim: None,
                    helper: None,
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

/// The label of a binary operation, total ones canonical, any other one
/// kept ordered by operator text.
fn label_binary(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    bin: &syn::ExprBinary,
    consumer: usize,
    locals: &mut usize,
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (left, mut sites) = label_expr(renamer, env, &bin.left, consumer, locals, orders);
    let (right, r_sites) = label_expr(renamer, env, &bin.right, consumer, locals, orders);
    sites.extend(r_sites);
    let label = if bin.attrs.is_empty()
        && let Some(reduced) = true_comparison(bin.op, &left, &right)
    {
        reduced
    } else {
        match total_binary(bin.op, &left, &right) {
            Some(TotalBinary::Ordered {
                tag: '∧' | '∨', ..
            }) if !bin.attrs.is_empty() => Lbl::Other(op_text(bin.op), vec![left, right]),
            Some(TotalBinary::Ordered { tag, reverse, prim }) => {
                let (lo, hi) = if reverse.is_some() {
                    (right, left)
                } else {
                    (left, right)
                };
                Lbl::Op(tag, Box::new(lo), Box::new(hi), prim)
            }
            Some(TotalBinary::Flat(op, prim)) => {
                let mut leaves = Vec::new();
                collect_leaves(op, prim, left, &mut leaves);
                collect_leaves(op, prim, right, &mut leaves);
                flat_label(op, prim, leaves)
            }
            None => {
                let tag = if is_assign_op(bin.op) {
                    "ao".into()
                } else {
                    op_text(bin.op)
                };
                Lbl::Other(tag, vec![left, right])
            }
        }
    };
    if label.prim().is_some() {
        sites.clear();
        value_sites(&label, &mut sites);
    }
    (label, sites)
}

fn value_sites(label: &Lbl, sites: &mut Vec<Site>) {
    match label {
        Lbl::Ref(value) => sites.push(Site {
            port: value.port.clone(),
            ctx: Ctx::Value,
        }),
        Lbl::Not(inner) => value_sites(inner, sites),
        Lbl::Op(_, left, right, _) => {
            value_sites(left, sites);
            value_sites(right, sites);
        }
        Lbl::Flat(_, leaves, _) | Lbl::Call(_, leaves, _) => {
            for leaf in leaves {
                value_sites(leaf, sites);
            }
        }
        _ => {}
    }
}

/// The label of a tuple, its elements in source order.
fn label_tuple(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    elems: &syn::punctuated::Punctuated<syn::Expr, syn::token::Comma>,
    consumer: usize,
    locals: &mut usize,
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let mut children = Vec::new();
    let mut sites = Vec::new();
    for element in elems {
        let (label, e_sites) = label_expr(renamer, env, element, consumer, locals, orders);
        children.push(label);
        sites.extend(e_sites);
    }
    (Lbl::Other("t".into(), children), sites)
}

/// Label a call with its resolved callee and independently total ordered arguments.
fn label_call(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    call: &syn::ExprCall,
    consumer: usize,
    locals: &mut usize,
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (callee, mut sites) = label_expr(renamer, env, &call.func, consumer, locals, orders);
    let mut args: Vec<Lbl> = Vec::new();
    for arg in &call.args {
        let (label, a_sites) = label_expr(renamer, env, arg, consumer, locals, orders);
        args.push(label);
        sites.extend(a_sites);
    }
    if let syn::Expr::Path(path) = call.func.as_ref()
        && path.qself.is_none()
        && let Some(helper) = renamer.helper_id(env, &path.path)
        && let Some(result) = total_helper(renamer, helper, &args, call)
        && let Some(key) = renamer.helper_identity(helper)
    {
        let identity = Lbl::Ref(Value {
            port: Port::Free(key),
            prim: None,
            helper: None,
        });
        let label = Lbl::Call(Box::new(identity), args, result);
        sites.clear();
        value_sites(&label, &mut sites);
        return (label, sites);
    }
    let mut children = vec![callee];
    children.extend(args);
    (Lbl::Other("c".into(), children), sites)
}

fn total_helper(
    renamer: &Renamer,
    helper: crate::scope::HelperId,
    args: &[Lbl],
    call: &syn::ExprCall,
) -> Option<PrimTy> {
    let facts = renamer.helper_table.get(helper)?;
    if !call.attrs.is_empty() || !crate::drift::expr_attrs(&call.func).is_empty() {
        return None;
    }
    if call.args.len() != facts.params.len() {
        return None;
    }
    for (arg, param) in args.iter().zip(&facts.params) {
        if arg.prim() != Some(*param) {
            return None;
        }
    }
    Some(facts.result)
}

/// The label of a method call, the receiver's sites marked on its storage,
/// its arguments in operand order.
fn label_method(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    method: &syn::ExprMethodCall,
    consumer: usize,
    locals: &mut usize,
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (receiver, receiver_sites) =
        label_expr(renamer, env, &method.receiver, consumer, locals, orders);
    let mut sites: Vec<_> = receiver_sites
        .into_iter()
        .map(|site| Site {
            ctx: Ctx::Storage,
            ..site
        })
        .collect();
    let mut children = vec![Lbl::Lit(method.method.to_string()), receiver];
    for arg in &method.args {
        let (label, a_sites) = label_expr(renamer, env, arg, consumer, locals, orders);
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (inner, inner_sites) = label_expr(renamer, env, &reference.expr, consumer, locals, orders);
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

/// A unary label with proven boolean negation pushed inward.
fn label_unary(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    unary: &syn::ExprUnary,
    consumer: usize,
    locals: &mut usize,
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (inner, sites) = label_expr(renamer, env, &unary.expr, consumer, locals, orders);
    let label = if matches!(unary.op, syn::UnOp::Not(_))
        && unary.attrs.is_empty()
        && inner.prim() == Some(PrimTy::Bool)
    {
        not_label(inner)
    } else {
        Lbl::Other("u".into(), vec![inner])
    };
    (label, sites)
}

/// The single unattributed, bounded value tail of a clean block.
pub(crate) fn scalar_tail(block: &syn::Block) -> Option<&syn::Expr> {
    let [syn::Stmt::Expr(tail, None)] = block.stmts.as_slice() else {
        return None;
    };
    if !crate::drift::expr_attrs(tail).is_empty() || !crate::algebra::within_bounds(tail) {
        return None;
    }
    Some(tail)
}

/// Select independently typed literal-condition tails or opposite boolean literals.
pub(crate) fn branch_value(cond: &Lbl, then: &Lbl, otherwise: &Lbl) -> Option<Lbl> {
    if let Lbl::Bool(selected) = cond {
        return (then.prim() == otherwise.prim() && then.prim().is_some()).then(|| {
            if *selected {
                then.clone()
            } else {
                otherwise.clone()
            }
        });
    }
    match (then, otherwise) {
        (Lbl::Bool(true), Lbl::Bool(false)) if cond.prim() == Some(PrimTy::Bool) => {
            Some(cond.clone())
        }
        (Lbl::Bool(false), Lbl::Bool(true)) if cond.prim() == Some(PrimTy::Bool) => {
            Some(not_label(cond.clone()))
        }
        _ => None,
    }
}

/// Strip unattributed wrappers and leading negations, retaining their parity.
pub(crate) fn condition_polarity(expr: &syn::Expr) -> (&syn::Expr, bool) {
    let mut current = expr;
    let mut negated = false;
    loop {
        let next = match current {
            syn::Expr::Unary(unary)
                if matches!(unary.op, syn::UnOp::Not(_))
                    && crate::drift::expr_attrs(current).is_empty() =>
            {
                negated = !negated;
                &unary.expr
            }
            syn::Expr::Group(group) if crate::drift::expr_attrs(current).is_empty() => &group.expr,
            _ => return (current, negated),
        };
        current = next;
    }
}

pub(crate) struct BranchScan {
    pub(crate) attributed: bool,
    pub(crate) depth: usize,
    pub(crate) expr_depth: usize,
    pub(crate) nodes: usize,
    pub(crate) valid: bool,
}

impl BranchScan {
    pub(crate) fn new() -> Self {
        Self {
            attributed: false,
            depth: 0,
            expr_depth: 0,
            nodes: 0,
            valid: true,
        }
    }
}

impl<'ast> Visit<'ast> for BranchScan {
    fn visit_attribute(&mut self, _: &'ast syn::Attribute) {
        self.attributed = true;
    }

    fn visit_block(&mut self, block: &'ast syn::Block) {
        let expr_depth = core::mem::replace(&mut self.expr_depth, 0);
        self.depth += 1;
        if !self.valid || self.depth > MAX_BLOCK_DEPTH {
            self.valid = false;
        } else {
            syn::visit::visit_block(self, block);
        }
        self.depth -= 1;
        self.expr_depth = expr_depth;
    }

    fn visit_stmt(&mut self, stmt: &'ast syn::Stmt) {
        self.nodes += 1;
        if !self.valid || self.nodes > MAX_REGION_NODES {
            self.valid = false;
            return;
        }
        syn::visit::visit_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'ast syn::Expr) {
        self.nodes += 1;
        self.expr_depth += 1;
        if !self.valid || self.nodes > MAX_REGION_NODES || self.expr_depth > MAX_BLOCK_DEPTH {
            self.valid = false;
        } else {
            syn::visit::visit_expr(self, expr);
        }
        self.expr_depth -= 1;
    }

    fn visit_item(&mut self, item: &'ast syn::Item) {
        self.nodes += 1;
        if !self.valid || self.nodes > MAX_REGION_NODES {
            self.valid = false;
            return;
        }
        syn::visit::visit_item(self, item);
    }
}

/// Whether both ordinary branches are unattributed and within the shared bounds.
pub(crate) fn branches_unattributed(
    attrs: &[syn::Attribute],
    then: &syn::Block,
    otherwise: &syn::ExprBlock,
) -> bool {
    if !attrs.is_empty() {
        return false;
    }
    let mut scan = BranchScan::new();
    scan.visit_block(then);
    scan.visit_expr_block(otherwise);
    scan.valid && !scan.attributed
}

/// Orient an unattributed conditional by its leading negation parity.
fn branch_orientation(r#if: &syn::ExprIf) -> Option<(&syn::Expr, &syn::Block, &syn::Block)> {
    let (positive, negated) = condition_polarity(&r#if.cond);
    if !negated || !crate::drift::expr_attrs(positive).is_empty() {
        return None;
    }
    let (_, otherwise) = r#if.else_branch.as_ref()?;
    let syn::Expr::Block(otherwise) = otherwise.as_ref() else {
        return None;
    };
    if !branches_unattributed(&r#if.attrs, &r#if.then_branch, otherwise) {
        return None;
    }
    Some((positive, &otherwise.block, &r#if.then_branch))
}

/// Prove the positive condition before orienting whole branches.
fn oriented_if<'r>(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    r#if: &'r syn::ExprIf,
    consumer: usize,
    locals: &mut usize,
    orders: Option<&RefCell<FieldOrders>>,
) -> Option<(Lbl, Vec<Site>, &'r syn::Block, &'r syn::Block)> {
    let (positive, then, otherwise) = branch_orientation(r#if)?;
    let saved = *locals;
    let (cond, cond_sites) = label_expr(renamer, env, positive, consumer, locals, orders);
    if cond.prim() != Some(PrimTy::Bool) {
        *locals = saved;
        return None;
    }
    Some((cond, cond_sites, then, otherwise))
}

/// Fold scalar branches only after proving their types independently.
fn boolean_branch_fold(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    consumer: usize,
    locals: &mut usize,
    orders: Option<&RefCell<FieldOrders>>,
    cond: &Lbl,
    branches: (&syn::Block, &syn::Block),
) -> Option<Lbl> {
    let (then, otherwise) = branches;
    let then_tail = scalar_tail(then)?;
    let otherwise_tail = scalar_tail(otherwise)?;
    let saved = *locals;
    let (then_label, _) = label_expr(renamer, env, then_tail, consumer, locals, orders);
    let (otherwise_label, _) = label_expr(renamer, env, otherwise_tail, consumer, locals, orders);
    let Some(value) = branch_value(cond, &then_label, &otherwise_label) else {
        *locals = saved;
        return None;
    };
    Some(value)
}

/// Label proven branch folds before allocating branch-local ports.
fn label_if_expr(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    r#if: &syn::ExprIf,
    consumer: usize,
    locals: &mut usize,
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    if let Some((cond, cond_sites, then, otherwise)) =
        oriented_if(renamer, env, r#if, consumer, locals, orders)
    {
        if let Some(value) = boolean_branch_fold(
            renamer,
            env,
            consumer,
            locals,
            orders,
            &cond,
            (then, otherwise),
        ) {
            let mut sites = Vec::new();
            value_sites(&value, &mut sites);
            return (value, sites);
        }
        let (then_label, then_sites) = label_block(renamer, env, then, consumer, locals, orders);
        let (otherwise_label, otherwise_sites) =
            label_block(renamer, env, otherwise, consumer, locals, orders);
        let mut sites = cond_sites;
        sites.extend(then_sites);
        sites.extend(otherwise_sites);
        return (
            Lbl::Other("if".into(), vec![cond, then_label, otherwise_label]),
            sites,
        );
    }
    let (cond, mut sites) = label_cond(renamer, env, &r#if.cond, consumer, locals, orders);
    let saved = *locals;
    let otherwise = r#if
        .else_branch
        .as_ref()
        .and_then(|(_, else_expr)| match else_expr.as_ref() {
            syn::Expr::Block(block) => Some(block),
            _ => None,
        });
    if cond.prim() == Some(PrimTy::Bool)
        && crate::drift::expr_attrs(&r#if.cond).is_empty()
        && let Some(otherwise) = otherwise
        && let Some(value) = boolean_branch_fold(
            renamer,
            env,
            consumer,
            locals,
            orders,
            &cond,
            (&r#if.then_branch, &otherwise.block),
        )
        && branches_unattributed(&r#if.attrs, &r#if.then_branch, otherwise)
    {
        let mut sites = Vec::new();
        value_sites(&value, &mut sites);
        return (value, sites);
    }
    *locals = saved;
    let (then_label, then_sites) =
        label_block(renamer, env, &r#if.then_branch, consumer, locals, orders);
    let mut children = vec![cond, then_label];
    sites.extend(then_sites);
    if let Some((_, else_expr)) = &r#if.else_branch {
        let (else_label, else_sites) =
            label_expr(renamer, env, else_expr, consumer, locals, orders);
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (iterable, mut sites) = label_expr(renamer, env, &for_loop.expr, consumer, locals, orders);
    let mut inner = env.clone();
    for name in pat_names(&for_loop.pat) {
        inner.insert(
            name,
            Value {
                port: Port::Local(*locals),
                prim: None,
                helper: None,
            },
        );
        *locals += 1;
    }
    let (body, body_sites) = label_block(renamer, &inner, &for_loop.body, consumer, locals, orders);
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (cond, mut sites) = label_cond(renamer, env, &r#while.cond, consumer, locals, orders);
    let (body, body_sites) = label_block(renamer, env, &r#while.body, consumer, locals, orders);
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (body, sites) = label_block(renamer, env, &r#loop.body, consumer, locals, orders);
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (scrutinee, mut sites) = label_expr(renamer, env, &r#match.expr, consumer, locals, orders);
    let mut children = vec![scrutinee];
    for arm in &r#match.arms {
        let mut inner = env.clone();
        for name in pat_names(&arm.pat) {
            inner.insert(
                name,
                Value {
                    port: Port::Local(*locals),
                    prim: None,
                    helper: None,
                },
            );
            *locals += 1;
        }
        let (guard_label, guard_sites) = match &arm.pat {
            syn::Pat::Guard(guard) => {
                label_expr(renamer, &inner, &guard.guard, consumer, locals, orders)
            }
            _ => (Lbl::Other("ng".into(), Vec::new()), Vec::new()),
        };
        sites.extend(guard_sites);
        let (body, body_sites) = label_expr(renamer, &inner, &arm.body, consumer, locals, orders);
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
    orders: Option<&RefCell<FieldOrders>>,
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
                    helper: None,
                },
            );
            *locals += 1;
        }
    }
    let (body, body_sites) = match &*closure.body {
        syn::Expr::Block(block) => {
            label_block(renamer, &inner, &block.block, consumer, locals, orders)
        }
        other => label_expr(renamer, &inner, other, consumer, locals, orders),
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let mut children = Vec::new();
    let mut sites = Vec::new();
    for element in &array.elems {
        let (label, e_sites) = label_expr(renamer, env, element, consumer, locals, orders);
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (element, mut sites) = label_expr(renamer, env, &repeat.expr, consumer, locals, orders);
    let (length, l_sites) = label_expr(renamer, env, &repeat.len, consumer, locals, orders);
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let mut children = Vec::new();
    let mut sites = Vec::new();
    if let Some(start) = &range.start {
        let (label, s_sites) = label_expr(renamer, env, start, consumer, locals, orders);
        children.push(label);
        sites.extend(s_sites);
    }
    if let Some(end) = &range.end {
        let (label, s_sites) = label_expr(renamer, env, end, consumer, locals, orders);
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (base, mut sites) = label_expr(renamer, env, &index_expr.expr, consumer, locals, orders);
    let (idx, i_sites) = label_expr(renamer, env, &index_expr.index, consumer, locals, orders);
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (base, sites) = label_expr(renamer, env, &field.base, consumer, locals, orders);
    (
        Lbl::Other(
            "f".into(),
            vec![base, Lbl::Lit(field.member.to_token_stream().to_string())],
        ),
        sites,
    )
}

/// The label of a struct expression, its fields with member identities in the admitted semantic order, and its update base use.
fn label_struct_expr(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    struct_expr: &syn::ExprStruct,
    consumer: usize,
    locals: &mut usize,
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let mut member_names: Vec<String> = Vec::with_capacity(struct_expr.fields.len());
    let mut labels: Vec<Option<Lbl>> = Vec::with_capacity(struct_expr.fields.len());
    let mut total: Vec<bool> = Vec::with_capacity(struct_expr.fields.len());
    let mut sites = Vec::new();
    let mut nodes = 2usize;
    for field in &struct_expr.fields {
        member_names.push(crate::structure::member_name(&field.member));
        let (label, f_sites) = label_expr(renamer, env, &field.expr, consumer, locals, orders);
        total.push(label.prim().is_some());
        nodes += 2 + label_nodes(&label);
        labels.push(Some(label));
        sites.extend(f_sites);
    }
    let mut base = None;
    if let Some(rest) = &struct_expr.rest {
        let (label, b_sites) = label_expr(renamer, env, rest, consumer, locals, orders);
        nodes += 1 + label_nodes(&label);
        sites.extend(b_sites);
        base = Some(label);
    }
    let order = crate::structure::named_field_order(struct_expr, &member_names, &total, nodes);
    if let Some(order) = &order
        && order
            .iter()
            .enumerate()
            .any(|(position, source)| position != *source)
    {
        permute_values(&mut member_names, order);
        permute_values(&mut labels, order);
    }
    let mut children = vec![Lbl::Lit(path_text(&struct_expr.path))];
    for (name, label) in member_names.into_iter().zip(labels) {
        children.push(Lbl::Other(
            "f".into(),
            vec![Lbl::Lit(name), label.expect("a labeled field")],
        ));
    }
    if let Some(base) = base {
        children.push(Lbl::Other("b".into(), vec![base]));
    }
    if let Some(cell) = orders
        && let Some(first) = struct_expr.fields.first()
    {
        cell.borrow_mut().insert(core::ptr::from_ref(first), order);
    }
    (Lbl::Other("st".into(), children), sites)
}

/// In-place cycle rotation of `values` under the destination to source `order`.
fn permute_values<T>(values: &mut [T], order: &[usize]) {
    let count = order.len();
    let mut settled = vec![false; count];
    for (start, source) in order.iter().copied().enumerate() {
        if settled[start] || source == start {
            settled[start] = true;
            continue;
        }
        let mut i = start;
        let mut j = source;
        while j != start {
            values.swap(i, j);
            settled[j] = true;
            i = j;
            j = order[i];
        }
        settled[start] = true;
    }
}

/// The label of a `let` expression, its scrutinee labeled.
fn label_let_expr(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    let_expr: &syn::ExprLet,
    consumer: usize,
    locals: &mut usize,
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (value, sites) = label_expr(renamer, env, &let_expr.expr, consumer, locals, orders);
    (Lbl::Other("le".into(), vec![value]), sites)
}

/// The label of an assignment expression, its sides in operand order.
fn label_assign(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    assign: &syn::ExprAssign,
    consumer: usize,
    locals: &mut usize,
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (left, mut sites) = label_expr(renamer, env, &assign.left, consumer, locals, orders);
    let (right, r_sites) = label_expr(renamer, env, &assign.right, consumer, locals, orders);
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    match value {
        Some(value) => {
            let (label, sites) = label_expr(renamer, env, value, consumer, locals, orders);
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (label, sites) = label_expr(renamer, env, &try_expr.expr, consumer, locals, orders);
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (label, sites) = label_block(renamer, env, block, consumer, locals, orders);
    (Lbl::Other("blk".into(), vec![label]), sites)
}

/// The label of an `await` expression, its operand labeled.
fn label_await(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    await_expr: &syn::ExprAwait,
    consumer: usize,
    locals: &mut usize,
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (label, sites) = label_expr(renamer, env, &await_expr.base, consumer, locals, orders);
    (Lbl::Other("aw".into(), vec![label]), sites)
}

/// The label of a cast expression, its operand and its target type text.
fn label_cast(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    cast: &syn::ExprCast,
    consumer: usize,
    locals: &mut usize,
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (value, sites) = label_expr(renamer, env, &cast.expr, consumer, locals, orders);
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let (label, inner_sites) = label_expr(renamer, env, &raw.expr, consumer, locals, orders);
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
    orders: Option<&RefCell<FieldOrders>>,
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
        let (label, a_sites) = label_expr(renamer, env, arg, consumer, locals, orders);
        children.push(label);
        sites.extend(a_sites);
    }
    if let Some((position, _)) = crate::drift::format_operand(&mac.path)
        && let Some(argument) = args.get(position)
    {
        if let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(lit),
            ..
        }) = argument
        {
            sites.extend(
                format_refs(&lit.value(), &args)
                    .into_iter()
                    .filter_map(|name| {
                        env.get(&name).map(|value| Site {
                            port: value.port.clone(),
                            ctx: Ctx::Value,
                        })
                    }),
            );
        } else {
            sites.extend(env.values().map(|value| Site {
                port: value.port.clone(),
                ctx: Ctx::Value,
            }));
        }
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    let mut inner = env.clone();
    renamer.helper_import_env(block, &mut inner);
    let mut children = Vec::new();
    let mut sites = Vec::new();
    for (position, stmt) in block.stmts.iter().enumerate() {
        let (label, stmt_sites) = statement_label(renamer, &inner, stmt, position, locals, orders);
        sites.extend(stmt_sites);
        if let syn::Stmt::Local(local) = stmt {
            let prim = if let_name(local).is_some() {
                label.prim()
            } else {
                None
            };
            for name in pat_names(&local.pat) {
                inner.insert(
                    name,
                    Value {
                        port: Port::Local(*locals),
                        prim,
                        helper: None,
                    },
                );
                *locals += 1;
            }
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    match stmt {
        syn::Stmt::Expr(expr, _) => label_expr(renamer, env, expr, consumer, locals, orders),
        syn::Stmt::Macro(stmt_macro) => {
            macro_label(renamer, env, &stmt_macro.mac, consumer, locals, orders)
        }
        syn::Stmt::Local(local) => match local.init.as_ref() {
            Some(init) if init.diverge.is_none() => {
                label_expr(renamer, env, &init.expr, consumer, locals, orders)
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
    orders: Option<&RefCell<FieldOrders>>,
) -> (Lbl, Vec<Site>) {
    match cond {
        syn::Expr::Let(let_expr) => {
            let (value, sites) = label_expr(renamer, env, &let_expr.expr, consumer, locals, orders);
            (Lbl::Other("lc".into(), vec![value]), sites)
        }
        other => label_expr(renamer, env, other, consumer, locals, orders),
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
    orders: Option<&RefCell<FieldOrders>>,
) -> Option<(String, Lbl, Vec<Site>)> {
    let name = let_name(local)?;
    let init = local.init.as_ref()?;
    if init.diverge.is_some() {
        return None;
    }
    let (label, sites) = label_expr(renamer, env, &init.expr, consumer, locals, orders);
    Some((name, label, sites))
}

/// The proven initializer type of `expr`, when it stays inside the total
/// primitive language.
fn total_init(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    expr: &syn::Expr,
    orders: Option<&RefCell<FieldOrders>>,
) -> Option<PrimTy> {
    let mut locals = 0usize;
    let (label, _) = label_expr(renamer, env, expr, 0, &mut locals, orders);
    label.prim()
}

/// Whether the tail root of `expr` is a clean read of proven values: a
/// proven literal, a proven reference, or a total expression over them,
/// a tuple of such.
fn tail_root(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    expr: &syn::Expr,
    orders: Option<&RefCell<FieldOrders>>,
) -> bool {
    match peel_paren(expr) {
        syn::Expr::Tuple(tuple) => tuple
            .elems
            .iter()
            .all(|element| tail_root(renamer, env, element, orders)),
        _ => total_init(renamer, env, expr, orders).is_some(),
    }
}

/// A color in the refinement palette, the rank of a distinct signature.
type Color = u32;

/// The complete structural signature a node refines over, its node
/// references replaced by the previous round's colors.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Sig<C> {
    /// The base operation, type and literal label, color-substituted.
    base: Lbl,
    /// The input neighbors' colors, in operand order.
    inputs: Vec<Color>,
    /// The full consumer descriptors, color-substituted, sorted.
    consumers: Vec<C>,
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
        Lbl::Not(inner) => Lbl::Not(Box::new(color_substitute(inner, colors))),
        Lbl::Op(op, left, right, prim) => Lbl::Op(
            *op,
            Box::new(color_substitute(left, colors)),
            Box::new(color_substitute(right, colors)),
            *prim,
        ),
        Lbl::Flat(op, leaves, prim) => {
            let mut leaves: Vec<_> = leaves
                .iter()
                .map(|leaf| color_substitute(leaf, colors))
                .collect();
            leaves.sort();
            Lbl::Flat(*op, leaves, *prim)
        }
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
        Lbl::Call(callee, args, prim) => Lbl::Call(
            Box::new(color_substitute(callee, colors)),
            args.iter()
                .map(|arg| color_substitute(arg, colors))
                .collect(),
            *prim,
        ),
        other => other.clone(),
    }
}

/// The palette assignment: the colors the distinct signatures sort to.
fn assign_palette<C: Ord>(sigs: &[Sig<C>]) -> Vec<Color> {
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

/// Substitute the future references of `lbl`, `map_future` building each
/// `Future` leaf.
fn future_substitute<F>(lbl: &Lbl, map_future: &F) -> Lbl
where
    F: Fn(usize, Option<PrimTy>) -> Lbl,
{
    match lbl {
        Lbl::Future(index, prim) => map_future(*index, *prim),
        Lbl::Not(inner) => Lbl::Not(Box::new(future_substitute(inner, map_future))),
        Lbl::Op(op, left, right, prim) => Lbl::Op(
            *op,
            Box::new(future_substitute(left, map_future)),
            Box::new(future_substitute(right, map_future)),
            *prim,
        ),
        Lbl::Flat(op, leaves, prim) => {
            let mut leaves = leaves
                .iter()
                .map(|leaf| future_substitute(leaf, map_future))
                .collect::<Vec<_>>();
            leaves.sort();
            Lbl::Flat(*op, leaves, *prim)
        }
        Lbl::Other(kind, children) => Lbl::Other(
            kind.clone(),
            children
                .iter()
                .map(|child| future_substitute(child, map_future))
                .collect(),
        ),
        Lbl::Call(callee, args, prim) => Lbl::Call(
            Box::new(future_substitute(callee, map_future)),
            args.iter()
                .map(|arg| future_substitute(arg, map_future))
                .collect(),
            *prim,
        ),
        other => other.clone(),
    }
}

/// Preserve current identities when remapping future references.
fn future_rank_substitute(lbl: &Lbl, ranks: &[usize]) -> Lbl {
    future_substitute(lbl, &|index, prim| Lbl::Future(ranks[index], prim))
}

/// Replace the future references of `lbl` with the referenced futures'
/// previous-round colors.
fn future_color_substitute(lbl: &Lbl, colors: &[Color]) -> Lbl {
    future_substitute(lbl, &|index, _| color_node(colors[index]))
}

fn future_refs(lbl: &Lbl, visit: &mut impl FnMut(usize)) {
    match lbl {
        Lbl::Future(index, _) => visit(*index),
        Lbl::Not(inner) => future_refs(inner, visit),
        Lbl::Op(_, left, right, _) => {
            future_refs(left, visit);
            future_refs(right, visit);
        }
        Lbl::Flat(_, children, _) | Lbl::Other(_, children) => {
            for child in children {
                future_refs(child, visit);
            }
        }
        Lbl::Call(callee, args, _) => {
            future_refs(callee, visit);
            for arg in args {
                future_refs(arg, visit);
            }
        }
        _ => {}
    }
}

/// Rank dependency values without identifying equal producer origins.
fn future_value_ranks(defs: &[Lbl]) -> Option<Vec<usize>> {
    let mut pending = vec![0usize; defs.len()];
    let mut readers = vec![Vec::new(); defs.len()];
    for (member, definition) in defs.iter().enumerate() {
        future_refs(definition, &mut |input| {
            pending[member] += 1;
            readers[input].push(member);
        });
    }
    let mut ready: Vec<_> = (0..defs.len())
        .filter(|&index| pending[index] == 0)
        .collect();
    let mut next = Vec::new();
    let mut labels = Vec::new();
    let mut ranks = vec![0; defs.len()];
    let mut next_rank = 0;
    let mut processed = 0;
    while !ready.is_empty() {
        labels.extend(
            ready
                .iter()
                .map(|&index| (index, future_rank_substitute(&defs[index], &ranks))),
        );
        labels.sort_unstable_by(|left, right| left.1.cmp(&right.1));
        let mut rank = next_rank;
        let mut previous = None;
        for (member, definition) in &labels {
            if previous != Some(definition) {
                rank = next_rank;
                next_rank += 1;
            }
            previous = Some(definition);
            ranks[*member] = rank;
            processed += 1;
        }
        labels.clear();
        for member in ready.drain(..) {
            for &reader in &readers[member] {
                pending[reader] -= 1;
                if pending[reader] == 0 {
                    next.push(reader);
                }
            }
        }
        core::mem::swap(&mut ready, &mut next);
    }
    (processed == defs.len()).then_some(ranks)
}

fn future_contexts(root: &Lbl, defs: &[Lbl], colors: &[Color]) -> Vec<Vec<Lbl>> {
    let mut contexts = vec![Vec::new(); defs.len()];
    let mut path = Vec::new();
    future_context_walk(
        root,
        &Lbl::Other("root".into(), Vec::new()),
        &mut path,
        &mut contexts,
    );
    for (member, definition) in defs.iter().enumerate() {
        future_context_walk(
            definition,
            &color_node(colors[member]),
            &mut path,
            &mut contexts,
        );
    }
    for context in &mut contexts {
        context.sort();
    }
    contexts
}

fn future_context_walk(node: &Lbl, owner: &Lbl, path: &mut Vec<Lbl>, contexts: &mut [Vec<Lbl>]) {
    match node {
        Lbl::Future(index, _) => {
            contexts[*index].push(Lbl::Other(
                "read".into(),
                vec![owner.clone(), Lbl::Other("path".into(), path.clone())],
            ));
        }
        Lbl::Not(inner) => {
            path.push(Lbl::Other(
                "not".into(),
                vec![Lbl::Int(0, Some(PrimTy::Bool))],
            ));
            future_context_walk(inner, owner, path, contexts);
            path.pop();
        }
        Lbl::Op(op, left, right, prim) => {
            for (position, child) in [left, right].into_iter().enumerate() {
                path.push(Lbl::Other(
                    op.to_string(),
                    vec![Lbl::Int(0, Some(*prim)), Lbl::Lit(position.to_string())],
                ));
                future_context_walk(child, owner, path, contexts);
                path.pop();
            }
        }
        Lbl::Flat(op, children, prim) => {
            path.push(Lbl::Other(op.to_string(), vec![Lbl::Int(0, Some(*prim))]));
            for child in children {
                future_context_walk(child, owner, path, contexts);
            }
            path.pop();
        }
        Lbl::Other(kind, children) => {
            for (position, child) in children.iter().enumerate() {
                path.push(Lbl::Other(
                    kind.clone(),
                    vec![Lbl::Lit(position.to_string())],
                ));
                future_context_walk(child, owner, path, contexts);
                path.pop();
            }
        }
        _ => {}
    }
}

/// Refinement colors summarize future values without equating origins.
fn future_summary(lbl: &Lbl) -> Lbl {
    match lbl {
        Lbl::Graph(root, definitions) => {
            let ranks =
                future_value_ranks(definitions).unwrap_or_else(|| vec![0; definitions.len()]);
            let mut definitions = definitions
                .iter()
                .map(|definition| future_rank_substitute(definition, &ranks))
                .collect::<Vec<_>>();
            definitions.sort();
            Lbl::Graph(Box::new(future_rank_substitute(root, &ranks)), definitions)
        }
        Lbl::Not(inner) => Lbl::Not(Box::new(future_summary(inner))),
        Lbl::Op(op, left, right, prim) => Lbl::Op(
            *op,
            Box::new(future_summary(left)),
            Box::new(future_summary(right)),
            *prim,
        ),
        Lbl::Flat(op, leaves, prim) => {
            let mut leaves = leaves.iter().map(future_summary).collect::<Vec<_>>();
            leaves.sort();
            Lbl::Flat(*op, leaves, *prim)
        }
        Lbl::Other(kind, children) => {
            Lbl::Other(kind.clone(), children.iter().map(future_summary).collect())
        }
        Lbl::Call(callee, args, prim) => Lbl::Call(
            Box::new(future_summary(callee)),
            args.iter().map(future_summary).collect(),
            *prim,
        ),
        other => other.clone(),
    }
}

/// Refine future definitions without spending a search state.
fn refine_future(defs: &[Lbl], root: &Lbl, seed: &[usize], marks: &[usize]) -> Vec<Color> {
    let mut colors: Vec<_> = seed
        .iter()
        .map(|&rank| Color::try_from(rank).expect("a bounded future color fits u32"))
        .collect();
    if find_future_tie(&colors).is_none() {
        return colors;
    }
    let mut passes = 0;
    loop {
        let previous = colors.clone();
        let contexts = future_contexts(root, defs, &previous);
        let mut sigs = Vec::with_capacity(defs.len());
        for (member, consumers) in contexts.into_iter().enumerate() {
            let base = future_color_substitute(&defs[member], &previous);
            let mark = marks.get(member).copied().unwrap_or(0);
            let base = if mark == 0 {
                base
            } else {
                Lbl::Other("I".into(), vec![Lbl::Lit(mark.to_string()), base])
            };
            sigs.push(Sig {
                base,
                inputs: Vec::new(),
                consumers,
                prev_color: previous[member],
            });
        }
        let next = assign_palette(&sigs);
        let stable = next == colors;
        colors = next;
        passes += 1;
        if stable || passes > defs.len() {
            break;
        }
    }
    colors
}

/// The first future color class with more than one member.
fn find_future_tie(colors: &[Color]) -> Option<Vec<usize>> {
    let mut groups: BTreeMap<Color, Vec<usize>> = BTreeMap::new();
    for (member, &color) in colors.iter().enumerate() {
        groups.entry(color).or_default().push(member);
    }
    groups.into_values().find(|members| members.len() >= 2)
}

fn spend_state(states: &mut usize) -> Option<()> {
    *states += 1;
    if *states > MAX_LABEL_STATES {
        return None;
    }
    Some(())
}

/// Minimize complete future identities within the shared region budget.
fn future_search(
    root: &Lbl,
    defs: &[Lbl],
    seed: &[usize],
    marks: &mut [usize],
    states: &mut usize,
    depth: usize,
) -> Option<Lbl> {
    spend_state(states)?;
    let colors = refine_future(defs, root, seed, marks);
    if let Some(members) = find_future_tie(&colors) {
        let mut best: Option<Lbl> = None;
        for member in members {
            marks[member] = depth + 1;
            let candidate = future_search(root, defs, seed, marks, states, depth + 1);
            marks[member] = 0;
            let candidate = candidate?;
            if best.as_ref().is_none_or(|winner| candidate < *winner) {
                best = Some(candidate);
            }
        }
        return best;
    }
    let mut order: Vec<_> = (0..defs.len()).collect();
    order.sort_unstable_by_key(|&index| colors[index]);
    let mut ranks = vec![0; defs.len()];
    for (position, &index) in order.iter().enumerate() {
        ranks[index] = position;
    }
    let root = future_rank_substitute(root, &ranks);
    let definitions = order
        .into_iter()
        .map(|index| future_rank_substitute(&defs[index], &ranks))
        .collect();
    Some(Lbl::Graph(Box::new(root), definitions))
}

pub(crate) fn item_order(definitions: &[Lbl], root: &Lbl) -> Option<Vec<usize>> {
    fn search(
        definitions: &[Lbl],
        root: &Lbl,
        seed: &[usize],
        marks: &mut [usize],
        states: &mut usize,
        depth: usize,
    ) -> Option<(Lbl, Vec<usize>)> {
        spend_state(states)?;
        let colors = refine_future(definitions, root, seed, marks);
        if let Some(members) = find_future_tie(&colors) {
            let mut best: Option<(Lbl, Vec<usize>)> = None;
            for member in members {
                marks[member] = depth + 1;
                let candidate = search(definitions, root, seed, marks, states, depth + 1);
                marks[member] = 0;
                let candidate = candidate?;
                if best.as_ref().is_none_or(|winner| candidate.0 < winner.0) {
                    best = Some(candidate);
                }
            }
            return best;
        }
        let mut order: Vec<_> = (0..definitions.len()).collect();
        order.sort_unstable_by_key(|&index| colors[index]);
        let mut ranks = vec![0; definitions.len()];
        for (position, &index) in order.iter().enumerate() {
            ranks[index] = position;
        }
        let candidate = Lbl::Graph(
            Box::new(future_rank_substitute(root, &ranks)),
            order
                .iter()
                .map(|&index| future_rank_substitute(&definitions[index], &ranks))
                .collect(),
        );
        Some((candidate, order))
    }
    let seed = vec![0; definitions.len()];
    search(
        definitions,
        root,
        &seed,
        &mut vec![0; definitions.len()],
        &mut 0,
        0,
    )
    .map(|(_, order)| order)
}

/// Share the region budget across complete future projections.
fn future_normalize(lbl: &Lbl, states: &mut usize) -> Option<Lbl> {
    match lbl {
        Lbl::Graph(root, definitions) => {
            let seed = future_value_ranks(definitions)?;
            let mut marks = vec![0; definitions.len()];
            future_search(root, definitions, &seed, &mut marks, states, 0)
        }
        other => Some(other.clone()),
    }
}

/// Refine the region's colors until their class pattern stops refining or
/// a size guard stops them. Per-state work, never a search state.
fn refine_to_point(
    inits: &[Lbl],
    deps: &[Vec<usize>],
    uses: &Uses,
    by_slot: &[Vec<usize>],
    n: usize,
) -> Vec<Color> {
    let keys: Vec<_> = uses
        .consumers
        .iter()
        .map(|consumer| (consumer.kind.to_string(), consumer.index.to_string()))
        .collect();
    let mut colors = vec![0u32; n];
    let mut passes = 0;
    loop {
        let previous = colors.clone();
        let summaries: Vec<_> = uses
            .consumers
            .iter()
            .map(|consumer| future_summary(&color_substitute(&consumer.label, &previous)))
            .collect();
        let mut sigs = Vec::with_capacity(n);
        for slot in 0..n {
            let mut consumers: Vec<_> = by_slot[slot]
                .iter()
                .map(|&entry| {
                    let id = uses.entries[entry].consumer.0;
                    (keys[id].0.as_str(), keys[id].1.as_str(), &summaries[id])
                })
                .collect();
            consumers.sort();
            let mut inputs: Vec<_> = deps[slot].iter().map(|&dep| previous[dep]).collect();
            inputs.sort_unstable();
            sigs.push(Sig {
                base: future_summary(&color_substitute(&inits[slot], &previous)),
                inputs,
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
                helper: None,
            }));
        }
        if let Some((region, slot)) = self.sealed[id] {
            return Some(Lbl::Ref(Value {
                port: Port::Sealed(region, slot),
                prim: value.prim,
                helper: None,
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

    fn labels(&mut self, labels: &[Lbl]) -> Option<Vec<Lbl>> {
        labels.iter().map(|label| self.label(label)).collect()
    }

    fn label(&mut self, lbl: &Lbl) -> Option<Lbl> {
        Some(match lbl {
            Lbl::Ref(value) => match value.port {
                Port::Node(id) => return self.reference(value, id),
                _ => Lbl::Ref(value.clone()),
            },
            Lbl::Not(inner) => Lbl::Not(Box::new(self.label(inner)?)),
            Lbl::Op(op, left, right, prim) => Lbl::Op(
                *op,
                Box::new(self.label(left)?),
                Box::new(self.label(right)?),
                *prim,
            ),
            Lbl::Flat(op, leaves, prim) => {
                let mut leaves = self.labels(leaves)?;
                leaves.sort();
                Lbl::Flat(*op, leaves, *prim)
            }
            Lbl::Other(kind, children) => Lbl::Other(kind.clone(), self.labels(children)?),
            Lbl::Call(callee, args, prim) => {
                Lbl::Call(Box::new(self.label(callee)?), self.labels(args)?, *prim)
            }
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
    uses: &Uses,
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
                && !uses.entries.iter().any(|use_| use_.node == nodes[slot])
                && !deps.iter().any(|readers| readers.contains(&slot))
        });
        if !twins {
            return Some(members);
        }
    }
    None
}

/// A complete region candidate preserves joint future sharing.
struct Candidate {
    declarations: Vec<Lbl>,
    consumers: Vec<(Color, ConsumerId)>,
    consumer_labels: Vec<Consumer>,
    context: Lbl,
    order: Vec<usize>,
}

impl Candidate {
    fn consumer_keys(&self) -> impl Iterator<Item = (u8, usize, Color, &Lbl)> {
        self.consumers.iter().map(|&(position, id)| {
            let consumer = &self.consumer_labels[id.0];
            (consumer.kind, consumer.index, position, &consumer.label)
        })
    }
}

struct RegionLabels<'a> {
    base: &'a [Lbl],
    nodes: &'a [usize],
    deps: &'a [Vec<usize>],
    uses: &'a Uses,
    by_slot: &'a [Vec<usize>],
    context: &'a Lbl,
}

fn complete_candidate(
    graph: &RegionLabels<'_>,
    order: Vec<usize>,
    states: &mut usize,
) -> Option<Candidate> {
    let RegionLabels {
        base: inits,
        nodes,
        uses,
        context,
        ..
    } = graph;
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
    let mut cache = vec![None; uses.consumers.len()];
    let mut consumer_labels = Vec::new();
    let mut consumers = Vec::with_capacity(uses.entries.len());
    for use_ in &uses.entries {
        let id = if let Some((id, cost)) = cache[use_.consumer.0] {
            // Each occurrence spends its complete search-state cost.
            for _ in 0..cost {
                spend_state(states)?;
            }
            id
        } else {
            let original = &uses.consumers[use_.consumer.0];
            let before = *states;
            let label = future_normalize(&color_substitute(&original.label, &positions), states)?;
            let id = ConsumerId(consumer_labels.len());
            consumer_labels.push(Consumer {
                kind: original.kind,
                index: original.index,
                label,
            });
            cache[use_.consumer.0] = Some((id, *states - before));
            id
        };
        consumers.push((positions[slots[&use_.node]], id));
    }
    consumers.sort_by(|&(position, id), &(other_position, other_id)| {
        let consumer = &consumer_labels[id.0];
        let other = &consumer_labels[other_id.0];
        (consumer.kind, consumer.index, position, &consumer.label).cmp(&(
            other.kind,
            other.index,
            other_position,
            &other.label,
        ))
    });
    let context = future_normalize(&color_substitute(context, &positions), states)?;
    Some(Candidate {
        declarations,
        consumers,
        consumer_labels,
        context,
        order,
    })
}

fn label_region_search(
    graph: &RegionLabels<'_>,
    inits: &[Lbl],
    states: &mut usize,
    depth: usize,
) -> Option<Candidate> {
    let RegionLabels {
        base,
        nodes,
        deps,
        uses,
        ..
    } = graph;
    spend_state(states)?;
    let colors = refine_to_point(inits, deps, uses, graph.by_slot, nodes.len());
    if let Some(members) = find_branchable_tie(&colors, inits, deps, uses, nodes) {
        let mut best: Option<Candidate> = None;
        for member in members {
            let mut marked = inits.to_vec();
            marked[member] = Lbl::Other(
                "I".into(),
                vec![Lbl::Lit(depth.to_string()), base[member].clone()],
            );
            let candidate = label_region_search(graph, &marked, states, depth + 1)?;
            if best.as_ref().is_none_or(|winner| {
                candidate
                    .declarations
                    .cmp(&winner.declarations)
                    .then_with(|| candidate.consumer_keys().cmp(winner.consumer_keys()))
                    .then_with(|| candidate.context.cmp(&winner.context))
                    .is_lt()
            }) {
                best = Some(candidate);
            }
        }
        return best;
    }
    let order = kahn_order(&colors, deps, nodes.len())?;
    complete_candidate(graph, order, states)
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

fn label_affected(lbl: &Lbl, affected: &[bool]) -> bool {
    match lbl {
        Lbl::Ref(Value {
            port: Port::Node(index),
            ..
        }) => affected[*index],
        Lbl::Not(inner) => label_affected(inner, affected),
        Lbl::Op(_, left, right, _) => {
            label_affected(left, affected) || label_affected(right, affected)
        }
        Lbl::Flat(_, children, _) | Lbl::Other(_, children) => {
            children.iter().any(|child| label_affected(child, affected))
        }
        Lbl::Call(callee, args, _) => {
            label_affected(callee, affected) || {
                args.iter().any(|arg| label_affected(arg, affected))
            }
        }
        _ => false,
    }
}

fn region_context(
    slots: &BTreeMap<usize, usize>,
    producers: &[Producer],
    uses: &Uses,
    sealed: &[Option<(usize, usize)>],
) -> Option<Lbl> {
    let mut affected = vec![false; producers.len()];
    let mut descendants = Vec::new();
    for (index, producer) in producers.iter().enumerate() {
        affected[index] = slots.contains_key(&index) || label_affected(&producer.init, &affected);
        if affected[index] && !slots.contains_key(&index) {
            descendants.push(Lbl::Ref(Value {
                port: Port::Node(index),
                prim: Some(producer.prim),
                helper: None,
            }));
        }
    }
    let mut roots: Vec<_> = uses
        .consumers
        .iter()
        .filter(|consumer| consumer.kind != USE_DOWN && label_affected(&consumer.label, &affected))
        .map(|consumer| (consumer.kind, consumer.index, &consumer.label))
        .collect();
    roots.sort();
    roots.dedup();
    let roots = roots
        .into_iter()
        .map(|(kind, index, label)| {
            Lbl::Other(
                "root".into(),
                vec![
                    Lbl::Lit(kind.to_string()),
                    Lbl::Lit(index.to_string()),
                    label.clone(),
                ],
            )
        })
        .collect();
    let context = Lbl::Other(
        "context".into(),
        vec![
            Lbl::Other("roots".into(), roots),
            Lbl::Flat('#', descendants, PrimTy::Bool),
        ],
    );
    region_label(&context, slots, producers, sealed)
}

/// Label one region's producers from their dependency edges and their
/// consumer uses, returning the canonical order of the region's slots or
/// `None` when a bound is crossed.
fn schedule_region(
    region: &Region,
    producers: &[Producer],
    uses: &Uses,
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
        for use_ in uses
            .entries
            .iter()
            .filter(|use_| slots.contains_key(&use_.node))
        {
            region_label(
                &uses.consumers[use_.consumer.0].label,
                &slots,
                producers,
                sealed,
            )?;
        }
        return Some((0..n).collect());
    }
    let inits: Vec<Lbl> = nodes
        .iter()
        .map(|&id| region_label(&producers[id].init, &slots, producers, sealed))
        .collect::<Option<_>>()?;
    let deps: Vec<Vec<usize>> = nodes.iter().map(|&id| producers[id].deps.clone()).collect();
    let mut region_uses = Uses::default();
    let mut projected = vec![None; uses.consumers.len()];
    let mut by_slot = vec![Vec::new(); n];
    for use_ in uses
        .entries
        .iter()
        .filter(|use_| slots.contains_key(&use_.node))
    {
        let consumer = if let Some(id) = projected[use_.consumer.0] {
            id
        } else {
            let original = &uses.consumers[use_.consumer.0];
            let label = region_label(&original.label, &slots, producers, sealed)?;
            let id = region_uses.insert(original.kind, original.index, label);
            projected[use_.consumer.0] = Some(id);
            id
        };
        by_slot[slots[&use_.node]].push(region_uses.entries.len());
        region_uses.entries.push(Use {
            node: use_.node,
            consumer,
        });
    }
    let context = region_context(&slots, producers, uses, sealed)?;
    let graph = RegionLabels {
        base: &inits,
        nodes,
        deps: &deps,
        uses: &region_uses,
        by_slot: &by_slot,
        context: &context,
    };
    let mut states = 0usize;
    label_region_search(&graph, &inits, &mut states, 0).map(|candidate| candidate.order)
}

pub(crate) fn record_all_opaque(renamer: &mut Renamer, block: &mut syn::Block) {
    renamer.facts.blocks.insert(
        core::ptr::from_ref(block),
        BlockProof {
            statements: vec![OPAQUE; block.stmts.len()],
            types: vec![None; block.stmts.len()],
            tail_types: Vec::new(),
        },
    );
    renamer.push_facts(BlockFacts {
        field_orders: BTreeMap::new(),
        plans: BTreeMap::new(),
        permutation: Vec::new(),
        original: None,
    });
}

struct BlockScan {
    env: BTreeMap<String, Value>,
    producers: Vec<Producer>,
    regions: Vec<Region>,
    uses: Uses,
    observed: BTreeSet<Port>,
    forbidden: BTreeSet<Port>,
    tags: Vec<u8>,
    tail_types: Vec<Option<PrimTy>>,
    locals: usize,
    unreachable: bool,
    tail_pending: bool,
    open: Option<usize>,
    over_budget: bool,
    field_orders: RefCell<FieldOrders>,
}

impl BlockScan {
    fn new(env: BTreeMap<String, Value>, locals: usize, count: usize) -> Self {
        Self {
            env,
            producers: Vec::new(),
            regions: Vec::new(),
            uses: Uses::default(),
            observed: BTreeSet::new(),
            forbidden: BTreeSet::new(),
            tags: vec![OPAQUE; count],
            tail_types: Vec::new(),
            locals,
            unreachable: false,
            tail_pending: false,
            open: None,
            over_budget: false,
            field_orders: RefCell::new(BTreeMap::new()),
        }
    }

    fn note(&mut self, sites: &[Site], kind: u8, index: usize, label: Lbl) {
        for site in sites {
            if site.ctx == Ctx::Storage {
                self.observed.insert(site.port.clone());
            }
            if site.ctx == Ctx::Storage || !matches!(kind, USE_DOWN | USE_TAIL) {
                self.forbidden.insert(site.port.clone());
            }
        }
        self.uses.note(
            sites,
            kind,
            index,
            label,
            &mut self.regions,
            &self.producers,
        );
    }

    /// A completed statement label past the node budget keeps the block
    /// opaque.
    fn budget(&mut self, label: &Lbl) {
        if label_nodes(label) > MAX_REGION_NODES {
            self.over_budget = true;
        }
    }

    fn statement(&mut self, renamer: &Renamer, index: usize, stmt: &syn::Stmt, last: bool) {
        if last
            && let syn::Stmt::Expr(expr, None) = stmt
            && (self.unreachable || tail_root(renamer, &self.env, expr, Some(&self.field_orders)))
        {
            self.tail_pending = true;
            return;
        }
        if self.unreachable {
            let (label, sites) = statement_label(
                renamer,
                &self.env,
                stmt,
                index,
                &mut self.locals,
                Some(&self.field_orders),
            );
            self.budget(&label);
            self.note(&sites, USE_UNREACHABLE, index, label);
            return;
        }
        if let syn::Stmt::Local(local) = stmt {
            self.local(renamer, index, stmt, local);
        } else {
            self.other(renamer, index, stmt);
        }
    }

    fn local(&mut self, renamer: &Renamer, index: usize, stmt: &syn::Stmt, local: &syn::Local) {
        if let Some((name, init, sites)) = let_producer(
            renamer,
            &self.env,
            local,
            index,
            &mut self.locals,
            Some(&self.field_orders),
        ) && init.prim().is_some()
        {
            self.budget(&init);
            self.producer(index, name, init, &sites);
            return;
        }
        let (label, sites) = statement_label(
            renamer,
            &self.env,
            stmt,
            index,
            &mut self.locals,
            Some(&self.field_orders),
        );
        self.budget(&label);
        self.note(&sites, USE_OPAQUE, index, label);
        for name in pat_names(&local.pat) {
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
                    helper: None,
                },
            );
            self.locals += 1;
            return;
        }
        let slot = self.regions[region].producers.len();
        let mut deps = Vec::new();
        let mut consumer = None;
        for site in sites {
            if let Port::Node(id) = &site.port {
                let producer_region = self.producers[*id].region;
                if producer_region == region {
                    deps.push(self.producers[*id].slot);
                } else {
                    let consumer = *consumer
                        .get_or_insert_with(|| self.uses.insert(USE_DOWN, 0, init.clone()));
                    self.uses.entries.push(Use {
                        node: *id,
                        consumer,
                    });
                }
            }
        }
        deps.sort_unstable();
        deps.dedup();
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
                helper: None,
            },
        );
    }

    fn other(&mut self, renamer: &Renamer, index: usize, stmt: &syn::Stmt) {
        let kind = stmt_kind(renamer, stmt);
        let (label, sites) = statement_label(
            renamer,
            &self.env,
            stmt,
            index,
            &mut self.locals,
            Some(&self.field_orders),
        );
        self.budget(&label);
        match kind {
            StmtKind::Barrier => {
                self.tags[index] = OBSERVATION;
                self.captures(index, stmt, &label);
                self.note(&sites, USE_OBSERVED, index, label);
            }
            StmtKind::Boundary(definite) => {
                self.tags[index] = BOUNDARY;
                self.note(&sites, USE_BOUNDARY, index, label);
                self.open = None;
                self.unreachable = definite;
                return;
            }
            StmtKind::Plain => self.note(&sites, USE_OPAQUE, index, label),
        }
        if let Some(target) = assign_target(stmt) {
            if let Some(value) = self.env.get(&target) {
                self.observed.insert(value.port.clone());
                self.forbidden.insert(value.port.clone());
                if let Port::Node(id) = &value.port {
                    self.regions[self.producers[*id].region].dead = true;
                }
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
                helper: None,
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
            if let Some(value) = self.env.get(&capture) {
                self.forbidden.insert(value.port.clone());
                if let Port::Node(node) = &value.port {
                    let consumer = self.uses.insert(
                        USE_CAPTURE,
                        index,
                        Lbl::Other(
                            "capture".into(),
                            vec![
                                Lbl::Int(
                                    u128::try_from(position).expect("a capture position fits u128"),
                                    None,
                                ),
                                label.clone(),
                            ],
                        ),
                    );
                    self.uses.entries.push(Use {
                        node: *node,
                        consumer,
                    });
                }
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
            let (label, sites) = label_expr(
                renamer,
                &self.env,
                expr,
                0,
                &mut self.locals,
                Some(&self.field_orders),
            );
            self.budget(&label);
            self.note(&sites, USE_UNREACHABLE, 0, label);
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
        let (label, sites) = label_expr(
            renamer,
            &self.env,
            expr,
            position,
            &mut self.locals,
            Some(&self.field_orders),
        );
        self.budget(&label);
        self.tail_types.push(label.prim());
        self.note(&sites, USE_TAIL, position, label);
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
                    active: !self.regions[producer.region].dead,
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

struct HelperCallScan<'a, 'env> {
    renamer: &'a Renamer<'env>,
    env: &'a BTreeMap<String, Value>,
    shadows: Vec<BTreeMap<String, Option<crate::scope::HelperId>>>,
    roots: BTreeSet<crate::scope::HelperId>,
}

impl<'ast> Visit<'ast> for HelperCallScan<'_, '_> {
    fn visit_block(&mut self, block: &'ast syn::Block) {
        let mut bindings = BTreeMap::new();
        for stmt in &block.stmts {
            if let syn::Stmt::Item(item) = stmt
                && let Some((namespace, ident)) = crate::alpha::item_binding(item)
            {
                let helper = match item {
                    syn::Item::Fn(function) => self.renamer.helper_declaration(&function.block),
                    syn::Item::Struct(item) if !matches!(item.fields, syn::Fields::Named(_)) => {
                        None
                    }
                    _ if namespace == Ns::Value => None,
                    _ => continue,
                };
                bindings.insert(crate::scope::ident_name(ident), helper);
            }
        }
        self.shadows.push(bindings);
        for stmt in &block.stmts {
            self.visit_stmt(stmt);
        }
        self.shadows.pop();
    }

    fn visit_local(&mut self, local: &'ast syn::Local) {
        if let Some(init) = &local.init {
            self.visit_expr(&init.expr);
            if let Some((_, otherwise)) = &init.diverge {
                self.visit_expr(otherwise);
            }
        }
        self.visit_pat(&local.pat);
    }

    fn visit_pat_ident(&mut self, pattern: &'ast syn::PatIdent) {
        self.shadows
            .last_mut()
            .expect("caller shadow frame")
            .insert(crate::scope::ident_name(&pattern.ident), None);
        syn::visit::visit_pat_ident(self, pattern);
    }

    fn visit_item(&mut self, _: &'ast syn::Item) {}

    fn visit_expr_closure(&mut self, closure: &'ast syn::ExprClosure) {
        self.shadows.push(BTreeMap::new());
        for input in &closure.inputs {
            self.visit_pat(input);
        }
        self.visit_expr(&closure.body);
        self.shadows.pop();
    }

    fn visit_arm(&mut self, arm: &'ast syn::Arm) {
        self.shadows.push(BTreeMap::new());
        self.visit_pat(&arm.pat);
        self.visit_expr(&arm.body);
        self.shadows.pop();
    }

    fn visit_expr_for_loop(&mut self, for_loop: &'ast syn::ExprForLoop) {
        self.visit_expr(&for_loop.expr);
        self.shadows.push(BTreeMap::new());
        self.visit_pat(&for_loop.pat);
        self.visit_block(&for_loop.body);
        self.shadows.pop();
    }

    fn visit_expr_if(&mut self, expr: &'ast syn::ExprIf) {
        self.shadows.push(BTreeMap::new());
        self.visit_expr(&expr.cond);
        self.visit_block(&expr.then_branch);
        self.shadows.pop();
        if let Some((_, otherwise)) = &expr.else_branch {
            self.visit_expr(otherwise);
        }
    }

    fn visit_expr_while(&mut self, expr: &'ast syn::ExprWhile) {
        self.shadows.push(BTreeMap::new());
        self.visit_expr(&expr.cond);
        self.visit_block(&expr.body);
        self.shadows.pop();
    }

    fn visit_expr_let(&mut self, expr: &'ast syn::ExprLet) {
        self.visit_expr(&expr.expr);
        self.visit_pat(&expr.pat);
    }

    fn visit_expr(&mut self, expr: &'ast syn::Expr) {
        if let syn::Expr::Call(call) = expr
            && let syn::Expr::Path(path) = call.func.as_ref()
            && path.qself.is_none()
        {
            let helper = if path.path.leading_colon.is_none() && path.path.segments.len() == 1 {
                let name = crate::scope::ident_name(&path.path.segments[0].ident);
                self.shadows
                    .iter()
                    .rev()
                    .find_map(|scope| scope.get(&name))
                    .copied()
                    .unwrap_or_else(|| self.renamer.helper_id(self.env, &path.path))
            } else {
                self.renamer.helper_id(self.env, &path.path)
            };
            if let Some(helper) = helper
                && self.renamer.helper_table.has_domain(helper)
            {
                self.roots.insert(helper);
            }
        }
        syn::visit::visit_expr(self, expr);
    }
}

fn called_helper_roots(
    renamer: &Renamer,
    env: &BTreeMap<String, Value>,
    block: &syn::Block,
) -> BTreeSet<crate::scope::HelperId> {
    let mut scan = HelperCallScan {
        renamer,
        env,
        shadows: Vec::new(),
        roots: BTreeSet::new(),
    };
    scan.visit_block(block);
    scan.roots
}

/// Resolve original statements and plan delayed region permutations.
#[expect(
    clippy::too_many_lines,
    reason = "the complete region proof and its atomic fallbacks stay together"
)]
pub(crate) fn analyze(renamer: &mut Renamer, block: &mut syn::Block) {
    let (frozen, mut seq, locals) = {
        let scope = renamer.scopes.last_mut().expect("a fn scope");
        (scope.frozen, scope.seq, scope.next_local)
    };
    if frozen || renamer.depth() > MAX_BLOCK_DEPTH {
        record_all_opaque(renamer, block);
        return;
    }
    if block.stmts.iter().any(|stmt| match stmt {
        syn::Stmt::Local(local) => local
            .init
            .as_ref()
            .is_some_and(|init| !crate::algebra::within_bounds(&init.expr)),
        syn::Stmt::Expr(expr, _) => !crate::algebra::within_bounds(expr),
        syn::Stmt::Item(_) | syn::Stmt::Macro(_) => false,
    }) {
        record_all_opaque(renamer, block);
        return;
    }
    let mut env = BTreeMap::new();
    renamer.frame_env(&mut env);
    renamer.helper_import_env(block, &mut env);
    let roots = called_helper_roots(renamer, &env, block);
    if !roots.is_empty() && renamer.helper_closure_sum(&roots) > MAX_REGION_NODES {
        record_all_opaque(renamer, block);
        return;
    }
    let original = match values::normalize(renamer, block) {
        values::Outcome::Unchanged => None,
        values::Outcome::Rewritten(original) => Some(original),
        values::Outcome::Exhausted => {
            record_all_opaque(renamer, block);
            return;
        }
    };
    let mut scan = BlockScan::new(env, locals, block.stmts.len());
    for (index, stmt) in block.stmts.iter().enumerate() {
        scan.statement(renamer, index, stmt, index + 1 == block.stmts.len());
    }
    scan.finish_tail(renamer, block);
    if scan.over_budget {
        if let Some(original) = original {
            *block = original;
        }
        record_all_opaque(renamer, block);
        let scope = renamer.scopes.last_mut().expect("a fn scope");
        scope.seq = seq;
        scope.next_local = scan.locals;
        return;
    }
    let region_seq: Vec<usize> = (0..scan.regions.len())
        .map(|_| {
            let current = seq;
            seq += 1;
            current
        })
        .collect();
    let frozen = original
        .as_ref()
        .map(|_| scan.regions.iter().filter(|region| region.dead).count());
    let orders = scan.schedule(&region_seq);
    if frozen.is_some_and(|count| count != scan.regions.iter().filter(|region| region.dead).count())
    {
        *block = original.expect("a rewritten block retains its original");
        record_all_opaque(renamer, block);
        return;
    }
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
    scope.observed.extend(
        scan.observed
            .into_iter()
            .filter(|port| !matches!(port, Port::Node(_))),
    );
    scope.observed.extend(
        plans
            .values()
            .filter(|plan| !plan.active)
            .map(|plan| plan.port.clone()),
    );
    renamer.facts.blocks.insert(
        core::ptr::from_ref(block),
        BlockProof {
            statements: scan.tags,
            types,
            tail_types: scan.tail_types,
        },
    );
    renamer.push_facts(BlockFacts {
        field_orders: scan.field_orders.into_inner(),
        plans,
        permutation,
        original,
    });
}
