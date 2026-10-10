//! Target-independent checked integer literal evaluation.

use alloc::boxed::Box;
use alloc::format;
use alloc::vec::Vec;
use syn::spanned::Spanned;
use syn::visit_mut::{self, VisitMut};

use crate::alpha::Renamer;
use crate::scope::{IntWidth, PrimTy, primitive_suffix};

const MAX_DEPTH: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
struct FixedInt {
    width: IntWidth,
    signed: bool,
}

impl FixedInt {
    fn suffix(self) -> &'static str {
        PrimTy::Int {
            width: self.width,
            signed: self.signed,
        }
        .suffix()
    }
}

#[derive(Clone, Copy)]
struct Integer {
    ty: FixedInt,
    bits: u128,
}

impl Integer {
    fn width(self) -> u32 {
        self.ty.width.bits()
    }

    fn signed(self) -> bool {
        self.ty.signed
    }

    fn mask(self) -> u128 {
        u128::MAX >> (128 - self.width())
    }

    fn signed_value(self) -> i128 {
        let sign = 1u128 << (self.width() - 1);
        let bits = if self.bits & sign == 0 {
            self.bits
        } else {
            self.bits | !self.mask()
        };
        i128::from_le_bytes(bits.to_le_bytes())
    }

    fn checked_signed(self, value: i128) -> Option<Self> {
        let limit = i128::MAX >> (128 - self.width());
        if value < -limit - 1 || value > limit {
            return None;
        }
        Some(Self {
            bits: u128::from_le_bytes(value.to_le_bytes()) & self.mask(),
            ..self
        })
    }

    fn checked_unsigned(self, value: u128) -> Option<Self> {
        (value <= self.mask()).then_some(Self {
            bits: value,
            ..self
        })
    }

    fn shift(self, right: Self, left: bool) -> Option<Self> {
        if right.signed() && right.signed_value() < 0 {
            return None;
        }
        let count = u32::try_from(right.bits).ok()?;
        if count >= self.width() {
            return None;
        }
        let bits = if left {
            self.bits.checked_shl(count)? & self.mask()
        } else if self.signed() {
            u128::from_le_bytes(self.signed_value().checked_shr(count)?.to_le_bytes()) & self.mask()
        } else {
            self.bits.checked_shr(count)?
        };
        Some(Self { bits, ..self })
    }

    fn binary(self, right: Self, op: BinaryOp) -> Option<Self> {
        let op = match op {
            BinaryOp::Shl => return self.shift(right, true),
            BinaryOp::Shr => return self.shift(right, false),
            BinaryOp::BitAnd => {
                return Some(Self {
                    bits: self.bits & right.bits,
                    ..self
                });
            }
            BinaryOp::BitOr => {
                return Some(Self {
                    bits: self.bits | right.bits,
                    ..self
                });
            }
            BinaryOp::BitXor => {
                return Some(Self {
                    bits: self.bits ^ right.bits,
                    ..self
                });
            }
            BinaryOp::Math(op) => op,
        };
        if self.signed() {
            let (left, right) = (self.signed_value(), right.signed_value());
            let minimum = -(i128::MAX >> (128 - self.width())) - 1;
            if matches!(op, MathOp::Div | MathOp::Rem) && left == minimum && right == -1 {
                return None;
            }
            let value = match op {
                MathOp::Add => left.checked_add(right),
                MathOp::Sub => left.checked_sub(right),
                MathOp::Mul => left.checked_mul(right),
                MathOp::Div => left.checked_div(right),
                MathOp::Rem => left.checked_rem(right),
            }?;
            self.checked_signed(value)
        } else {
            let value = match op {
                MathOp::Add => self.bits.checked_add(right.bits),
                MathOp::Sub => self.bits.checked_sub(right.bits),
                MathOp::Mul => self.bits.checked_mul(right.bits),
                MathOp::Div => self.bits.checked_div(right.bits),
                MathOp::Rem => self.bits.checked_rem(right.bits),
            }?;
            self.checked_unsigned(value)
        }
    }

    fn expression(self, span: proc_macro2::Span) -> syn::Expr {
        let negative = self.signed() && self.signed_value() < 0;
        let magnitude = if negative {
            self.signed_value().unsigned_abs()
        } else {
            self.bits
        };
        let literal = syn::Expr::Lit(syn::ExprLit {
            attrs: Vec::new(),
            lit: syn::Lit::Int(syn::LitInt::new(
                &format!("{magnitude}{}", self.ty.suffix()),
                span,
            )),
        });
        if negative {
            syn::Expr::Unary(syn::ExprUnary {
                attrs: Vec::new(),
                op: syn::UnOp::Neg(syn::token::Minus { spans: [span] }),
                expr: Box::new(literal),
            })
        } else {
            literal
        }
    }
}

#[derive(Clone, Copy)]
enum MathOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

#[derive(Clone, Copy)]
enum BinaryOp {
    Math(MathOp),
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

fn arithmetic(op: &syn::BinOp) -> Option<BinaryOp> {
    Some(match op {
        syn::BinOp::Add(_) => BinaryOp::Math(MathOp::Add),
        syn::BinOp::Sub(_) => BinaryOp::Math(MathOp::Sub),
        syn::BinOp::Mul(_) => BinaryOp::Math(MathOp::Mul),
        syn::BinOp::Div(_) => BinaryOp::Math(MathOp::Div),
        syn::BinOp::Rem(_) => BinaryOp::Math(MathOp::Rem),
        syn::BinOp::BitAnd(_) => BinaryOp::BitAnd,
        syn::BinOp::BitOr(_) => BinaryOp::BitOr,
        syn::BinOp::BitXor(_) => BinaryOp::BitXor,
        syn::BinOp::Shl(_) => BinaryOp::Shl,
        syn::BinOp::Shr(_) => BinaryOp::Shr,
        _ => return None,
    })
}

fn scalar_const(block: &syn::Block) -> Option<&syn::Expr> {
    match block.stmts.as_slice() {
        [syn::Stmt::Expr(expr, None)] => Some(expr),
        _ => None,
    }
}

enum Node<'a> {
    Literal(&'a syn::LitInt),
    Nested(&'a syn::Expr),
    Unary(&'a syn::ExprUnary),
    Binary(&'a syn::ExprBinary),
}

fn node(expr: &syn::Expr) -> Option<Node<'_>> {
    match expr {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(literal),
            ..
        }) => Some(Node::Literal(literal)),
        syn::Expr::Paren(paren) => Some(Node::Nested(&paren.expr)),
        syn::Expr::Group(group) => Some(Node::Nested(&group.expr)),
        syn::Expr::Const(constant) => scalar_const(&constant.block).map(Node::Nested),
        syn::Expr::Unary(unary) => Some(Node::Unary(unary)),
        syn::Expr::Binary(binary) => Some(Node::Binary(binary)),
        _ => None,
    }
}

fn closed(expr: &syn::Expr, depth: usize) -> bool {
    if depth >= MAX_DEPTH {
        return true;
    }
    match node(expr) {
        Some(Node::Literal(_)) => true,
        Some(Node::Nested(expr)) => closed(expr, depth + 1),
        Some(Node::Unary(unary)) if matches!(unary.op, syn::UnOp::Neg(_) | syn::UnOp::Not(_)) => {
            closed(&unary.expr, depth + 1)
        }
        Some(Node::Binary(binary)) if arithmetic(&binary.op).is_some() => {
            closed(&binary.left, depth + 1) && closed(&binary.right, depth + 1)
        }
        _ => false,
    }
}

fn integer_type(ty: PrimTy) -> Option<FixedInt> {
    match ty {
        PrimTy::Int { width, signed } => Some(FixedInt { width, signed }),
        PrimTy::Bool => None,
    }
}

#[derive(Clone, Copy)]
enum TypeProof {
    Unsuffixed,
    Fixed(FixedInt),
}

impl TypeProof {
    fn fixed(self) -> Option<FixedInt> {
        match self {
            Self::Unsuffixed => None,
            Self::Fixed(ty) => Some(ty),
        }
    }
}

fn proven_type(expr: &syn::Expr, expected: Option<FixedInt>, depth: usize) -> Option<TypeProof> {
    if depth >= MAX_DEPTH || !crate::drift::expr_attrs(expr).is_empty() {
        return None;
    }
    match node(expr)? {
        Node::Literal(literal) => {
            if literal.suffix().is_empty() {
                Some(expected.map_or(TypeProof::Unsuffixed, TypeProof::Fixed))
            } else {
                let ty = integer_type(primitive_suffix(literal.suffix())?)?;
                if expected.is_some_and(|expected| expected != ty) {
                    return None;
                }
                Some(TypeProof::Fixed(ty))
            }
        }
        Node::Nested(expr) => proven_type(expr, expected, depth + 1),
        Node::Unary(unary) => proven_type(&unary.expr, expected, depth + 1),
        Node::Binary(binary) => {
            let left = proven_type(&binary.left, expected, depth + 1)?;
            if matches!(binary.op, syn::BinOp::Shl(_) | syn::BinOp::Shr(_)) {
                proven_type(&binary.right, None, depth + 1)?.fixed()?;
                return Some(left);
            }
            let right = proven_type(&binary.right, expected, depth + 1)?;
            match (left, right) {
                (TypeProof::Fixed(left), TypeProof::Fixed(right)) if left != right => None,
                (TypeProof::Fixed(ty), _) | (_, TypeProof::Fixed(ty)) => Some(TypeProof::Fixed(ty)),
                (TypeProof::Unsuffixed, TypeProof::Unsuffixed) => Some(TypeProof::Unsuffixed),
            }
        }
    }
}

fn literal(expr: &syn::Expr) -> Option<&syn::LitInt> {
    match expr {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(literal),
            ..
        }) => Some(literal),
        syn::Expr::Group(group) => literal(&group.expr),
        _ => None,
    }
}

fn evaluate(expr: &syn::Expr, ty: FixedInt, depth: usize) -> Option<Integer> {
    let model = Integer { ty, bits: 0 };
    match node(expr)? {
        Node::Literal(literal) => {
            let magnitude = literal.base10_parse::<u128>().ok()?;
            if model.signed() {
                model.checked_signed(i128::try_from(magnitude).ok()?)
            } else {
                model.checked_unsigned(magnitude)
            }
        }
        Node::Nested(expr) => evaluate(expr, ty, depth + 1),
        Node::Unary(unary) => {
            if matches!(unary.op, syn::UnOp::Neg(_))
                && model.signed()
                && let Some(literal) = literal(&unary.expr)
            {
                let magnitude = literal.base10_parse::<u128>().ok()?;
                let limit = 1u128 << (model.width() - 1);
                if magnitude > limit {
                    return None;
                }
                return Some(Integer {
                    bits: magnitude.wrapping_neg() & model.mask(),
                    ..model
                });
            }
            let value = evaluate(&unary.expr, ty, depth + 1)?;
            match unary.op {
                syn::UnOp::Neg(_) if value.signed() => {
                    value.checked_signed(value.signed_value().checked_neg()?)
                }
                syn::UnOp::Not(_) => Some(Integer {
                    bits: !value.bits & value.mask(),
                    ..value
                }),
                _ => None,
            }
        }
        Node::Binary(binary) => {
            let left = evaluate(&binary.left, ty, depth + 1)?;
            let right_ty = if matches!(binary.op, syn::BinOp::Shl(_) | syn::BinOp::Shr(_)) {
                proven_type(&binary.right, None, depth + 1)?.fixed()?
            } else {
                ty
            };
            left.binary(
                evaluate(&binary.right, right_ty, depth + 1)?,
                arithmetic(&binary.op)?,
            )
        }
    }
}

pub(crate) fn normalize_expr(
    renamer: &Renamer<'_>,
    expr: &mut syn::Expr,
    expected: Option<PrimTy>,
) {
    Normalizer {
        renamer,
        expected: expected.and_then(integer_type),
        contextual: true,
        depth: 0,
    }
    .visit_expr_mut(expr);
}

pub(crate) fn normalize_block(renamer: &Renamer<'_>, block: &mut syn::Block) {
    Normalizer {
        renamer,
        expected: None,
        contextual: true,
        depth: 0,
    }
    .visit_block_mut(block);
}

struct Normalizer<'a, 'env> {
    renamer: &'a Renamer<'env>,
    expected: Option<FixedInt>,
    contextual: bool,
    depth: usize,
}

impl VisitMut for Normalizer<'_, '_> {
    fn visit_expr_mut(&mut self, expr: &mut syn::Expr) {
        if self.depth >= MAX_DEPTH || !crate::drift::expr_attrs(expr).is_empty() {
            return;
        }
        if closed(expr, 0) {
            if let Some(ty) = proven_type(expr, self.expected, 0).and_then(TypeProof::fixed)
                && let Some(value) = evaluate(expr, ty, 0)
            {
                if matches!(expr, syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(lit), .. }) if lit.suffix() == ty.suffix())
                {
                    return;
                }
                *expr = value.expression(expr.span());
            }
            return;
        }
        let expected = self.expected.take();
        self.depth += 1;
        visit_mut::visit_expr_mut(self, expr);
        self.depth -= 1;
        self.expected = expected;
    }

    fn visit_block_mut(&mut self, block: &mut syn::Block) {
        if self.depth >= MAX_DEPTH || crate::schedule::unknown_macro_reaches(self.renamer, block) {
            return;
        }
        let contextual = self.contextual;
        self.contextual &= !block.stmts.iter().any(|stmt| {
            matches!(stmt, syn::Stmt::Item(syn::Item::Use(_)))
                || (self.depth != 0 && matches!(stmt, syn::Stmt::Item(_)))
        });
        self.depth += 1;
        for stmt in &mut block.stmts {
            self.visit_stmt_mut(stmt);
        }
        self.depth -= 1;
        self.contextual = contextual;
    }

    fn visit_local_mut(&mut self, local: &mut syn::Local) {
        if !local.attrs.is_empty() {
            return;
        }
        if let Some(init) = &mut local.init {
            let expected = self.expected;
            self.expected = if self.contextual {
                match &local.pat {
                    syn::Pat::Type(typed) => {
                        self.renamer.param_prim(&typed.ty).and_then(integer_type)
                    }
                    _ => None,
                }
            } else {
                None
            };
            if !matches!(local.pat, syn::Pat::Type(_))
                || self.expected.is_some()
                || !closed(&init.expr, 0)
            {
                self.visit_expr_mut(&mut init.expr);
            }
            self.expected = expected;
            if let Some((_, diverge)) = &mut init.diverge {
                self.visit_expr_mut(diverge);
            }
        }
        self.visit_pat_mut(&mut local.pat);
    }

    fn visit_type_mut(&mut self, ty: &mut syn::Type) {
        if crate::drift::type_attrs(ty).is_empty() {
            visit_mut::visit_type_mut(self, ty);
        }
    }

    fn visit_generic_argument_mut(&mut self, arg: &mut syn::GenericArgument) {
        visit_mut::visit_generic_argument_mut(self, arg);
        if let syn::GenericArgument::Const(expr) = arg {
            crate::drift::fold_scalar_argument_block(expr);
        }
    }

    fn visit_item_mut(&mut self, _: &mut syn::Item) {}
    fn visit_macro_mut(&mut self, _: &mut syn::Macro) {}
    fn visit_attribute_mut(&mut self, _: &mut syn::Attribute) {}
}
