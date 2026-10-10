//! Primitive value laws over resolved, bounded expression trees.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use syn::spanned::Spanned as _;
use syn::visit::{self, Visit};
use syn::visit_mut::{self, VisitMut};

use crate::alpha::Renamer;
use crate::schedule::{self, BlockFacts, Lbl, Port, TotalBinary, Value};
use crate::scope::PrimTy;

pub(crate) fn within_bounds(expr: &syn::Expr) -> bool {
    struct Bound {
        depth: usize,
        nodes: usize,
        valid: bool,
    }
    impl<'ast> Visit<'ast> for Bound {
        fn visit_expr(&mut self, expr: &'ast syn::Expr) {
            if !self.valid {
                return;
            }
            self.nodes += 1;
            self.depth += 1;
            if self.depth > schedule::MAX_BLOCK_DEPTH || self.nodes > schedule::MAX_REGION_NODES {
                self.valid = false;
            } else {
                visit::visit_expr(self, expr);
            }
            self.depth -= 1;
        }
        fn visit_block(&mut self, _: &'ast syn::Block) {}
    }
    let mut bound = Bound {
        depth: 0,
        nodes: 0,
        valid: true,
    };
    bound.visit_expr(expr);
    bound.valid
}

pub(crate) fn normalize(renamer: &Renamer<'_>, block: &mut syn::Block, facts: &BlockFacts) {
    if !facts.can_rewrite() {
        return;
    }
    let mut env = BTreeMap::new();
    renamer.frame_env(&mut env);
    let mut walker = Walker {
        env,
        locals: 0,
        dedup: true,
    };
    for stmt in &mut block.stmts {
        let definite = matches!(
            schedule::stmt_kind(renamer, stmt),
            schedule::StmtKind::Boundary(true)
        );
        match stmt {
            syn::Stmt::Local(local) => {
                let plan = facts.plan(&local.pat);
                if local.attrs.is_empty()
                    && plan.is_none_or(|plan| plan.active)
                    && let Some(init) = &mut local.init
                    && init.diverge.is_none()
                {
                    walker.visit_expr_mut(&mut init.expr);
                }
                for name in schedule::pat_names(&local.pat) {
                    let value = if let Some(plan) = plan {
                        Value {
                            port: plan.port.clone(),
                            prim: if plan.active { plan.prim } else { None },
                        }
                    } else {
                        walker.locals += 1;
                        Value {
                            port: Port::Local(walker.locals),
                            prim: None,
                        }
                    };
                    walker.env.insert(name, value);
                }
            }
            syn::Stmt::Expr(expr, _) => {
                walker.visit_expr_mut(expr);
                if let Some(name) = schedule::assign_target(stmt) {
                    walker.locals += 1;
                    walker.env.insert(
                        name,
                        Value {
                            port: Port::Local(walker.locals),
                            prim: None,
                        },
                    );
                }
            }
            syn::Stmt::Item(_) | syn::Stmt::Macro(_) => {}
        }
        if definite {
            break;
        }
    }
}

struct Walker {
    env: BTreeMap<String, Value>,
    locals: usize,
    dedup: bool,
}

fn empty_expr() -> syn::Expr {
    syn::Expr::Verbatim(proc_macro2::TokenStream::new())
}

impl Walker {
    fn scalar_in_place(&self, expr: &mut syn::Expr) -> Option<Lbl> {
        let owned = core::mem::replace(expr, empty_expr());
        let (normalized, label) = self.scalar(owned);
        *expr = normalized;
        label
    }

    fn scalar(&self, expr: syn::Expr) -> (syn::Expr, Option<Lbl>) {
        if !crate::drift::expr_attrs(&expr).is_empty() {
            return (expr, None);
        }
        match expr {
            syn::Expr::Binary(mut binary) => {
                let left = self.scalar_in_place(&mut binary.left);
                let right = self.scalar_in_place(&mut binary.right);
                let (Some(left), Some(right)) = (left, right) else {
                    return (syn::Expr::Binary(binary), None);
                };
                match schedule::total_binary(&binary.op, &left, &right) {
                    Some(TotalBinary::Flat(op, prim)) => {
                        let span = binary.span();
                        let mut pieces = Vec::new();
                        extend(op, prim, left, *binary.left, &mut pieces);
                        extend(op, prim, right, *binary.right, &mut pieces);
                        pieces.sort_by(|a, b| a.0.cmp(&b.0));
                        if self.dedup && op != '^' {
                            pieces.dedup_by(|a, b| a.0 == b.0);
                        }
                        fold_literals(op, prim, &mut pieces, span);
                        if pieces.len() == 1 {
                            let (label, expr) = pieces.pop().expect("a single operand");
                            return (expr, Some(label));
                        }
                        let count = pieces.len();
                        let mut labels = Vec::with_capacity(count);
                        let expression = {
                            let mut expressions = pieces.into_iter().map(|(label, expr)| {
                                labels.push(label);
                                expr
                            });
                            emit(op, &mut expressions, count, span)
                        };
                        (expression, Some(Lbl::Flat(op, labels, prim)))
                    }
                    Some(TotalBinary::Ordered { tag, swap, prim }) => {
                        let (left, right) = if swap {
                            core::mem::swap(&mut binary.left, &mut binary.right);
                            binary.op = match tag {
                                '<' => syn::BinOp::Lt(syn::token::Lt::default()),
                                '≤' => syn::BinOp::Le(syn::token::Le::default()),
                                _ => unreachable!("only ordering comparisons reverse"),
                            };
                            (right, left)
                        } else {
                            (left, right)
                        };
                        (
                            syn::Expr::Binary(binary),
                            Some(Lbl::Op(tag, Box::new(left), Box::new(right), prim)),
                        )
                    }
                    None => (syn::Expr::Binary(binary), None),
                }
            }
            syn::Expr::Paren(paren) => self.scalar(*paren.expr),
            syn::Expr::Group(group) => self.scalar(*group.expr),
            expr @ (syn::Expr::Lit(_) | syn::Expr::Path(_) | syn::Expr::Unary(_)) => {
                let label = schedule::scalar_leaf(&self.env, &expr);
                (expr, label)
            }
            expr => (expr, None),
        }
    }
}

impl VisitMut for Walker {
    fn visit_expr_mut(&mut self, expr: &mut syn::Expr) {
        if !crate::drift::expr_attrs(expr).is_empty() {
            return;
        }
        match expr {
            syn::Expr::Binary(_)
            | syn::Expr::Lit(_)
            | syn::Expr::Path(_)
            | syn::Expr::Paren(_)
            | syn::Expr::Group(_) => {
                let owned = core::mem::replace(expr, empty_expr());
                *expr = self.scalar(owned).0;
            }
            syn::Expr::Reference(reference) => {
                let dedup = core::mem::replace(&mut self.dedup, false);
                self.visit_expr_mut(&mut reference.expr);
                self.dedup = dedup;
            }
            syn::Expr::RawAddr(raw) => {
                let dedup = core::mem::replace(&mut self.dedup, false);
                self.visit_expr_mut(&mut raw.expr);
                self.dedup = dedup;
            }
            syn::Expr::Block(_)
            | syn::Expr::Const(_)
            | syn::Expr::Unsafe(_)
            | syn::Expr::Async(_)
            | syn::Expr::TryBlock(_)
            | syn::Expr::Closure(_)
            | syn::Expr::Match(_)
            | syn::Expr::If(_)
            | syn::Expr::While(_)
            | syn::Expr::ForLoop(_)
            | syn::Expr::Loop(_)
            | syn::Expr::Let(_)
            | syn::Expr::Macro(_)
            | syn::Expr::Verbatim(_) => {}
            _ => visit_mut::visit_expr_mut(self, expr),
        }
    }

    fn visit_block_mut(&mut self, _block: &mut syn::Block) {}
}

fn extend(op: char, prim: PrimTy, label: Lbl, expr: syn::Expr, out: &mut Vec<(Lbl, syn::Expr)>) {
    match label {
        Lbl::Flat(inner_op, labels, inner_prim) if inner_op == op && inner_prim == prim => {
            let mut expressions = Vec::with_capacity(labels.len());
            decompose(op, expr, &mut expressions);
            assert_eq!(
                labels.len(),
                expressions.len(),
                "one expression per operand"
            );
            out.extend(labels.into_iter().zip(expressions));
        }
        label => out.push((label, expr)),
    }
}

fn bitwise_tag(op: &syn::BinOp) -> Option<char> {
    match op {
        syn::BinOp::BitAnd(_) => Some('&'),
        syn::BinOp::BitOr(_) => Some('|'),
        syn::BinOp::BitXor(_) => Some('^'),
        _ => None,
    }
}

fn decompose(op: char, expr: syn::Expr, out: &mut Vec<syn::Expr>) {
    match expr {
        syn::Expr::Binary(binary) if bitwise_tag(&binary.op) == Some(op) => {
            decompose(op, *binary.left, out);
            decompose(op, *binary.right, out);
        }
        expr => out.push(expr),
    }
}

fn fold_literals(
    op: char,
    prim: PrimTy,
    pieces: &mut Vec<(Lbl, syn::Expr)>,
    span: proc_macro2::Span,
) {
    let literal = |label: &Lbl| matches!(label, Lbl::Int(_, _) | Lbl::Bool(_));
    if pieces.iter().filter(|piece| literal(&piece.0)).count() < 2 {
        return;
    }
    let mut acc = None;
    pieces.retain(|piece| {
        if !literal(&piece.0) {
            return true;
        }
        acc = Some(match acc.take() {
            None => piece.0.clone(),
            Some(left) => schedule::combine_literal(op, prim, left, piece.0.clone()),
        });
        false
    });
    let label = acc.expect("at least two literal operands");
    let bits = match label {
        Lbl::Int(bits, _) => bits,
        Lbl::Bool(value) => u128::from(value),
        _ => unreachable!("a literal subset yields a literal"),
    };
    pieces.push((label, crate::constants::literal_expr(prim, bits, span)));
    pieces.sort_by(|a, b| a.0.cmp(&b.0));
}

fn emit(
    op: char,
    expressions: &mut impl Iterator<Item = syn::Expr>,
    count: usize,
    span: proc_macro2::Span,
) -> syn::Expr {
    if count == 1 {
        return expressions
            .next()
            .expect("an operand for each counted leaf");
    }
    let left = emit(op, expressions, count / 2, span);
    let right = emit(op, expressions, count - count / 2, span);
    let op = match op {
        '&' => syn::BinOp::BitAnd(syn::token::And { spans: [span] }),
        '|' => syn::BinOp::BitOr(syn::token::Or { spans: [span] }),
        '^' => syn::BinOp::BitXor(syn::token::Caret { spans: [span] }),
        _ => unreachable!("only primitive bitwise chains are emitted"),
    };
    syn::Expr::Binary(syn::ExprBinary {
        attrs: Vec::new(),
        left: Box::new(left),
        op,
        right: Box::new(right),
    })
}
