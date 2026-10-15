//! Named field and literal alternative reordering under the existing primitive proof.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::alpha::Renamer;
use crate::schedule::{self, MAX_REGION_NODES};
use crate::scope::ident_name;

/// Orders proven-total named fields within the source budget.
pub(crate) fn named_field_order(
    expr: &syn::ExprStruct,
    member_names: &[String],
    total: &[bool],
    nodes: usize,
) -> Option<Vec<usize>> {
    let count = expr.fields.len();
    if nodes > MAX_REGION_NODES || !expr.attrs.is_empty() || total.len() != count || count < 2 {
        return None;
    }
    if expr
        .rest
        .as_ref()
        .is_some_and(|rest| !crate::drift::expr_attrs(rest).is_empty())
    {
        return None;
    }
    let mut entries: Vec<(&str, usize)> = Vec::with_capacity(count);
    for (position, field) in expr.fields.iter().enumerate() {
        if !field.attrs.is_empty() || !total[position] {
            return None;
        }
        let syn::Member::Named(_) = &field.member else {
            return None;
        };
        entries.push((member_names[position].as_str(), position));
    }
    entries.sort_by(|left, right| left.0.cmp(right.0));
    let mut previous: Option<&str> = None;
    for (name, _) in &entries {
        if previous.is_some_and(|kept| kept == *name) {
            return None;
        }
        previous = Some(name);
    }
    Some(entries.into_iter().map(|(_, position)| position).collect())
}

/// Rejected cache entries never trigger another proof.
pub(crate) fn named_fields(renamer: &Renamer<'_>, expr: &mut syn::ExprStruct) {
    let Some(first) = expr.fields.first() else {
        return;
    };
    if let Some(record) = renamer
        .last_block_facts()
        .and_then(|facts| facts.field_order(first))
    {
        if let schedule::FieldOrder::Admitted(order) = record {
            reorder_fields(expr, order);
        }
        return;
    }
    if crate::algebra::within_struct_bounds(expr)
        && let Some(order) = out_of_block_order(renamer, expr)
    {
        reorder_fields(expr, &order);
    }
}

fn out_of_block_order(renamer: &Renamer<'_>, expr: &syn::ExprStruct) -> Option<Vec<usize>> {
    let mut env: BTreeMap<String, schedule::Value> = BTreeMap::new();
    renamer.frame_env(&mut env);
    let mut locals = 0usize;
    let mut member_names: Vec<String> = Vec::with_capacity(expr.fields.len());
    let mut total: Vec<bool> = Vec::with_capacity(expr.fields.len());
    let mut nodes = 2usize;
    for field in &expr.fields {
        member_names.push(member_name(&field.member));
        let (label, _) = schedule::label_expr(renamer, &env, &field.expr, 0, &mut locals, None);
        nodes += 2 + schedule::label_nodes(&label);
        total.push(label.prim().is_some());
    }
    if let Some(rest) = &expr.rest {
        let (label, _) = schedule::label_expr(renamer, &env, rest, 0, &mut locals, None);
        nodes += 1 + schedule::label_nodes(&label);
    }
    named_field_order(expr, &member_names, &total, nodes)
}

/// The semantic name of a struct member, its raw spelling removed.
pub(crate) fn member_name(member: &syn::Member) -> String {
    match member {
        syn::Member::Named(ident) => ident_name(ident),
        syn::Member::Unnamed(index) => format!("{}", index.index),
    }
}

fn reorder_fields(expr: &mut syn::ExprStruct, order: &[usize]) {
    permute_pairs(&mut expr.fields, order);
}

fn permute_pairs<T, P>(pairs: &mut syn::punctuated::Punctuated<T, P>, order: &[usize]) {
    if order
        .iter()
        .enumerate()
        .all(|(position, source)| position == *source)
    {
        return;
    }
    let mut slots: Vec<&mut T> = pairs
        .pairs_mut()
        .map(syn::punctuated::Pair::into_value)
        .collect();
    let mut settled = vec![false; order.len()];
    for (start, source) in order.iter().copied().enumerate() {
        if settled[start] || source == start {
            settled[start] = true;
            continue;
        }
        let mut current = start;
        let mut next = source;
        while next != start {
            let (lower, upper) = slots.split_at_mut(current.max(next));
            core::mem::swap(&mut *lower[current.min(next)], &mut *upper[0]);
            settled[next] = true;
            current = next;
            next = order[current];
        }
        settled[start] = true;
    }
}

/// Literal sorting preserves multiplicity and suffixes.
pub(crate) fn literal_alternatives(arm: &mut syn::Arm) {
    if !arm.attrs.is_empty() || matches!(arm.pat, syn::Pat::Guard(_)) {
        return;
    }
    let syn::Pat::Or(orbit) = &mut arm.pat else {
        return;
    };
    let case_count = orbit.cases.len();
    if !(2..=MAX_REGION_NODES).contains(&case_count) {
        return;
    }
    let mut keys: Vec<(u8, String)> = Vec::with_capacity(case_count);
    for case in &orbit.cases {
        let syn::Pat::Lit(pat_lit) = case else {
            return;
        };
        if !pat_lit.attrs.is_empty() {
            return;
        }
        match literal_key(&pat_lit.lit) {
            Some(key) => keys.push(key),
            None => return,
        }
    }
    let mut indexed: Vec<(usize, &(u8, String))> = keys.iter().enumerate().collect();
    indexed.sort_by(|left, right| left.1.cmp(right.1));
    let mut in_place = true;
    for (position, (source, _)) in indexed.iter().enumerate() {
        if *source != position {
            in_place = false;
            break;
        }
    }
    if in_place {
        return;
    }
    let order: Vec<usize> = indexed.into_iter().map(|(source, _)| source).collect();
    permute_pairs(&mut orbit.cases, &order);
}

/// Literal keys retain their kind, sign, payload and suffix.
fn literal_key(lit: &syn::Lit) -> Option<(u8, String)> {
    let (rank, token) = match lit {
        syn::Lit::Int(value) => (0, value.token()),
        syn::Lit::Float(value) => (1, value.token()),
        syn::Lit::Bool(value) => {
            return Some((2, String::from(if value.value { "true" } else { "false" })));
        }
        syn::Lit::Char(value) => (3, value.token()),
        syn::Lit::Byte(value) => (4, value.token()),
        syn::Lit::Str(value) => (5, value.token()),
        syn::Lit::ByteStr(value) => (6, value.token()),
        _ => return None,
    };
    let text = token.to_string();
    let key = if let Some(absolute) = text.strip_prefix('-') {
        let canonical = crate::drift::canonical_literal(absolute.parse().ok()?).to_string();
        format!("-{canonical}")
    } else {
        crate::drift::canonical_literal(token).to_string()
    };
    Some((rank, key))
}
