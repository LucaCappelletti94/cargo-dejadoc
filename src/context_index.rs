//! Source-only metadata extraction for indexed context records.

use crate::cfg::{path_claims, render_conjunction};
use crate::reuse::ContextAccess;
use crate::{ContextSite, TargetScan};
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use proc_macro2::Span;
use quote::ToTokens;
use syn::ext::IdentExt;
use syn::visit::Visit;

/// One indexed lexical context, recorded without a token floor.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IndexedContext {
    /// The canonical form text of the context.
    pub form: String,
    /// The canonical weighted-token count.
    pub tokens: usize,
    /// The original source range.
    pub site: ContextSite,
    /// The exact lexical module path of the context.
    pub module: String,
    /// The owning compilation target name.
    pub crate_name: String,
    /// The owning target root source path, workspace-relative.
    pub crate_root: String,
    /// Source-established accessibility of the context.
    pub access: ContextAccess,
    /// The established external route, for accessible whole functions and methods.
    #[serde(default)]
    pub api_path: Option<String>,
    /// Conditional-compilation requirements retained from source.
    pub conditions: Vec<String>,
    /// Source-established extraction constraints and unresolved facts.
    pub extraction_notes: Vec<String>,
}

/// The indexed contexts of `target`, one record per context and module membership.
#[must_use]
pub fn extract(
    target: &TargetScan,
    root: &str,
    scan_generated: bool,
    generated_markers: &[String],
) -> Vec<IndexedContext> {
    let layout = crate_layout(target);
    let model = CrateModel::new(target, &layout);
    let crate_root = workspace_relative(target.files.first().map_or("", |f| f.path.as_str()), root);
    let mut out = Vec::new();
    for file in target
        .files
        .iter()
        .filter(|file| scan_generated || !crate::is_generated(&file.text, generated_markers))
    {
        let path = workspace_relative(&file.path, root);
        let memberships = layout
            .memberships
            .get(&workspace_relative(&file.path, ""))
            .expect("a layout file");
        let meta = context_meta(&file.parsed);
        let mut extractor = Extractor {
            model: &model,
            library: target.library,
            name: &target.name,
            path: &path,
            crate_root: &crate_root,
            ascii: file.text.is_ascii(),
            offset: SourceOffset::new(&file.parsed, &file.text),
            text: &file.text,
            meta: &meta,
            seen: BTreeSet::new(),
            out: &mut out,
        };
        syn_canon::contexts(&file.parsed, &mut |context, canonical| {
            extractor.context(&context, canonical, memberships);
        });
    }
    out
}

/// The text `parse_file` dropped before tokenizing, for `byte_range` offsets.
struct SourceOffset {
    /// Bytes removed by shebang preprocessing.
    shebang: usize,
    /// Bytes removed by the leading BOM, 0 or 3.
    bom: usize,
}

impl SourceOffset {
    fn new(parsed: &syn::File, text: &str) -> Self {
        let shebang = parsed.shebang.as_ref().map_or(0, String::len);
        let bom = usize::from(text.starts_with('\u{feff}')) * '\u{feff}'.len_utf8();
        SourceOffset { shebang, bom }
    }

    /// The original-text index of a `byte_range` offset.
    fn origin(&self, offset: usize) -> usize {
        offset + self.shebang + self.bom
    }
}

/// The 1-based byte column of an original-text index, exclusive for ends.
fn byte_column(text: &str, offset: usize) -> usize {
    offset - text[..offset].rfind('\n').map_or(0, |newline| newline + 1) + 1
}

/// One file's extraction state, the callback closure's capture set.
struct Extractor<'a> {
    model: &'a CrateModel,
    library: bool,
    name: &'a str,
    path: &'a str,
    crate_root: &'a str,
    ascii: bool,
    offset: SourceOffset,
    text: &'a str,
    meta: &'a BTreeMap<SpanKey, Meta>,
    seen: BTreeSet<(usize, u32, usize, u32, usize)>,
    out: &'a mut Vec<IndexedContext>,
}

impl Extractor<'_> {
    fn context(
        &mut self,
        context: &syn_canon::Context,
        canonical: syn_canon::CanonicalForm,
        memberships: &[Membership],
    ) {
        let tokens = canonical.body_units();
        let form = canonical.into_key();
        let (start, end) = (context.span.start(), context.span.end());
        let (column, end_column) = if self.ascii {
            (start.column + 1, end.column + 1)
        } else {
            let bytes = context.span.byte_range();
            (
                byte_column(self.text, self.offset.origin(bytes.start)),
                byte_column(self.text, self.offset.origin(bytes.end)),
            )
        };
        let line = u32::try_from(start.line).unwrap_or(u32::MAX);
        let end_line = u32::try_from(end.line).unwrap_or(u32::MAX);
        let key_span = span_key(context.span);
        let chain = self
            .meta
            .get(&key_span)
            .map_or(Vec::new(), |m| m.chain.clone());
        let clf = Classifier {
            model: self.model,
            name: self.name,
            library: self.library,
        };
        for (membership_index, membership) in memberships.iter().enumerate() {
            let key = (membership_index, line, column, end_line, end_column);
            if !self.seen.insert(key) {
                continue;
            }
            let branch = render_alternatives(&membership.alternatives);
            let shadowed = !membership.alternatives.is_empty()
                && membership
                    .alternatives
                    .iter()
                    .all(|alternative| alternative.iter().any(|condition| condition == "false"));
            let (access, api_path, conditions, notes) = classify(
                &clf,
                &membership.module,
                branch.as_deref(),
                shadowed,
                self.meta
                    .get(&key_span)
                    .expect("the source walk records every emitted context"),
                context.item,
            );
            let prefix = join_path(
                core::iter::once(self.name).chain(membership.module.iter().map(String::as_str)),
            );
            let item = join_path([prefix.as_str(), context.item]);
            let module = join_path(
                [prefix.as_str()]
                    .into_iter()
                    .chain(chain.iter().map(String::as_str)),
            );
            self.out.push(IndexedContext {
                form: form.clone(),
                tokens,
                site: ContextSite {
                    file: self.path.to_string(),
                    line,
                    column,
                    end: end_line,
                    end_column,
                    item,
                    kind: context.kind,
                },
                module,
                crate_name: self.name.to_string(),
                crate_root: self.crate_root.to_string(),
                access,
                api_path,
                conditions,
                extraction_notes: notes,
            });
        }
    }
}

/// The crate and target inputs shared by the whole-body route classifiers.
struct Classifier<'a> {
    model: &'a CrateModel,
    name: &'a str,
    library: bool,
}

/// The accessibility and external route of a whole function or method.
fn classify(
    clf: &Classifier,
    membership: &[String],
    branch: Option<&str>,
    shadowed: bool,
    meta: &Meta,
    item: &str,
) -> (ContextAccess, Option<String>, Vec<String>, Vec<String>) {
    let mut conditions: Vec<String> = meta.cfg.clone();
    if let Some(branch) = branch {
        conditions.push(branch.to_string());
    }
    conditions.sort_unstable();
    conditions.dedup();
    if shadowed {
        return (
            ContextAccess::Unresolved,
            None,
            conditions,
            alloc::vec![
                "the source is shadowed by an earlier path provider and never linked".to_string()
            ],
        );
    }
    let module_rel = join_path(membership.iter().chain(meta.chain.iter()));
    let fn_name = item.rsplit("::").next().unwrap_or(item).to_string();
    let notes = Vec::new();
    match &meta.flavor {
        Flavor::Embedded => (ContextAccess::Embedded, None, conditions, notes),
        Flavor::Free { visible, nested } => {
            let path = join_path([module_rel.as_str(), fn_name.as_str()]);
            classify_item(clf, *nested, &path, *visible, conditions)
        }
        Flavor::Method {
            self_type,
            visible,
            nested,
        } => classify_method(
            clf,
            &module_rel,
            self_type,
            *visible,
            *nested,
            &fn_name,
            conditions,
        ),
        Flavor::TraitDefault {
            trait_name,
            trait_visible,
            nested,
        } => {
            let path = join_path([module_rel.as_str(), trait_name.as_str()]);
            classify_trait_default(clf, *nested, &path, *trait_visible, &fn_name, conditions)
        }
    }
}

/// The route of a whole free function body.
fn classify_item(
    clf: &Classifier,
    nested: bool,
    path: &str,
    visible: Vis,
    conditions: Vec<String>,
) -> (ContextAccess, Option<String>, Vec<String>, Vec<String>) {
    let mut notes = Vec::new();
    if !clf.library {
        notes.push(not_established());
        return (ContextAccess::Unresolved, None, conditions, notes);
    }
    if nested {
        notes.push(nested_note());
        return (ContextAccess::InternalFunction, None, conditions, notes);
    }
    match item_route(clf.model, path, visible == Vis::Public) {
        Established::Public {
            route,
            conditions: carried,
        } => {
            let conditions = merge_conditions(clf.model, conditions, carried, &route);
            (
                ContextAccess::PublicFunction,
                Some(qualify(clf.name, route)),
                conditions,
                notes,
            )
        }
        Established::Internal => (ContextAccess::InternalFunction, None, conditions, notes),
        Established::Unknown(reason) => {
            notes.push(reason);
            (ContextAccess::Unresolved, None, conditions, notes)
        }
    }
}

/// The route of a whole method body, decided by the receiver.
fn classify_method(
    clf: &Classifier,
    module_rel: &str,
    self_type: &str,
    visible: Vis,
    nested: bool,
    fn_name: &str,
    conditions: Vec<String>,
) -> (ContextAccess, Option<String>, Vec<String>, Vec<String>) {
    let mut notes = Vec::new();
    if !clf.library {
        notes.push(not_established());
        return (ContextAccess::Unresolved, None, conditions, notes);
    }
    if nested {
        notes.push(nested_note());
        return (ContextAccess::InternalMethod, None, conditions, notes);
    }
    if visible != Vis::Public {
        return (ContextAccess::InternalMethod, None, conditions, notes);
    }
    match receiver_route(clf.model, module_rel, self_type) {
        Established::Public {
            route,
            conditions: carried,
        } => (
            ContextAccess::PublicMethod,
            Some(qualify(clf.name, join_path([route.as_str(), fn_name]))),
            merge_conditions(clf.model, conditions, carried, &route),
            notes,
        ),
        Established::Internal => {
            notes.push(format!("the receiver {self_type} is internal to the crate"));
            (ContextAccess::InternalMethod, None, conditions, notes)
        }
        Established::Unknown(reason) => {
            notes.push(reason);
            (ContextAccess::Unresolved, None, conditions, notes)
        }
    }
}

/// The route of a whole trait default body, following the trait.
fn classify_trait_default(
    clf: &Classifier,
    nested: bool,
    path: &str,
    trait_visible: Vis,
    fn_name: &str,
    conditions: Vec<String>,
) -> (ContextAccess, Option<String>, Vec<String>, Vec<String>) {
    let mut notes = Vec::new();
    if !clf.library {
        notes.push(not_established());
        return (ContextAccess::Unresolved, None, conditions, notes);
    }
    if nested {
        notes.push(nested_note());
        return (ContextAccess::InternalMethod, None, conditions, notes);
    }
    match item_route(clf.model, path, trait_visible == Vis::Public) {
        Established::Public {
            route,
            conditions: carried,
        } => (
            ContextAccess::PublicMethod,
            Some(qualify(clf.name, join_path([route.as_str(), fn_name]))),
            merge_conditions(clf.model, conditions, carried, &route),
            notes,
        ),
        Established::Internal => (ContextAccess::InternalMethod, None, conditions, notes),
        Established::Unknown(reason) => {
            notes.push(reason);
            (ContextAccess::Unresolved, None, conditions, notes)
        }
    }
}

fn not_established() -> String {
    "the target is not a library, so external accessibility is not established".to_string()
}

fn nested_note() -> String {
    "a nested function body is never externally reachable".to_string()
}

/// The established route of an item as public, internal, or unestablished.
enum Established {
    /// The known external route, target-relative, with the conditions its resolution carried.
    Public {
        route: String,
        conditions: Vec<String>,
    },
    /// The source establishes that no external route exists.
    Internal,
    /// Source evidence does not settle the route, with the reason.
    Unknown(String),
}

fn item_route(model: &CrateModel, path: &str, visible: bool) -> Established {
    if model.chain_visible(path) && visible {
        return Established::Public {
            route: path.to_string(),
            conditions: Vec::new(),
        };
    }
    if visible && let Some((route, conditions)) = reexport_route(model, path) {
        return Established::Public { route, conditions };
    }
    if model.resolved && model.chain_known(path) {
        return Established::Internal;
    }
    if let Some(module) = first_unknown_module(model, path) {
        return Established::Unknown(format!(
            "module {module} is not found in the scanned source"
        ));
    }
    let module = model
        .first_unresolved_module()
        .expect("an unresolved target records an unresolved module");
    Established::Unknown(format!(
        "module {module} is not found in the scanned source"
    ))
}

/// The first module of `path` missing from the model.
fn first_unknown_module(model: &CrateModel, path: &str) -> Option<String> {
    module_segments(path)
        .into_iter()
        .find(|module| !model.modules.contains_key(module))
}

/// The base conditions plus every condition on the chosen route.
fn merge_conditions(
    model: &CrateModel,
    base: Vec<String>,
    carried: Vec<String>,
    route: &str,
) -> Vec<String> {
    let mut out = base;
    out.extend(carried);
    out.extend(route_path_conditions(model, route));
    out.sort_unstable();
    out.dedup();
    out
}

/// The cfg conditions of the modules and terminal type on `route`, plus the
/// branch conditions of any module whose claims do not cover every case.
fn route_path_conditions(model: &CrateModel, route: &str) -> Vec<String> {
    let mut out = Vec::new();
    for module in module_segments(route) {
        if let Some(info) = model.modules.get(&module) {
            out.extend(info.cfg.iter().cloned());
            if let Some(claim) = render_alternatives(&info.claims) {
                out.push(claim);
            }
        }
    }
    if let Some(def) = model.types.get(route) {
        out.extend(def.cfg.iter().cloned());
    }
    out
}

/// The text of a disjunction of claim alternatives, `None` when no condition
/// applies or an alternative is unconditional.
fn render_alternatives(alternatives: &[Vec<String>]) -> Option<String> {
    let mut rendered = Vec::new();
    for alternative in alternatives {
        let text = render_conjunction(alternative);
        if text.is_empty() {
            return None;
        }
        if !rendered.iter().any(|existing| existing == &text) {
            rendered.push(text);
        }
    }
    for text in &rendered {
        if rendered
            .iter()
            .any(|other| other == &format!("not({text})"))
        {
            return None;
        }
    }
    match rendered.len() {
        0 => None,
        1 => rendered.into_iter().next(),
        _ => Some(format!("any({})", rendered.join(", "))),
    }
}

/// The re-exported route of an item, through a bare `pub use` on a visible chain of the item
/// itself or of a module above it.
fn reexport_route(model: &CrateModel, path: &str) -> Option<(String, Vec<String>)> {
    for (module, bindings) in &model.bindings {
        if !model.module_chain_visible(module) {
            continue;
        }
        for binding in bindings.iter().filter(|b| b.visible) {
            let Some(target) = resolve_use_target(model, module, &binding.segments, binding.rooted)
            else {
                continue;
            };
            let route = match binding.alias.as_deref() {
                Some(alias) if target == path => qualified(module, alias),
                Some(alias) => match reexported_rest(model, &target, path) {
                    Some(rest) => qualified(module, &qualified(alias, rest)),
                    None => continue,
                },
                None => match reexported_rest(model, &target, path) {
                    Some(rest) => qualified(module, rest),
                    None => continue,
                },
            };
            return Some(reexport_with_conditions(model, binding, path, route));
        }
    }
    None
}

/// The part of `path` below the module `target`, when every module in it is visible.
fn reexported_rest<'a>(model: &CrateModel, target: &str, path: &'a str) -> Option<&'a str> {
    let rest = path.strip_prefix(target)?.strip_prefix("::")?;
    if rest.is_empty() || !model.modules.contains_key(target) {
        return None;
    }
    module_segments(rest)
        .iter()
        .all(|below| {
            model
                .modules
                .get(&qualified(target, below))
                .is_some_and(|info| info.visible && info.resolved)
        })
        .then_some(rest)
}

/// A re-export route with the cfg conditions of its declaration and the
/// conditions of the item's actual location.
fn reexport_with_conditions(
    model: &CrateModel,
    binding: &Binding,
    location: &str,
    route: String,
) -> (String, Vec<String>) {
    let mut conditions = binding.cfg.clone();
    conditions.extend(route_path_conditions(model, &route));
    conditions.extend(route_path_conditions(model, location));
    (route, conditions)
}

/// The receiver's established route for a written self type.
fn receiver_route(model: &CrateModel, module: &str, self_type: &str) -> Established {
    let Some(base) = bare_receiver(self_type) else {
        return Established::Unknown(format!(
            "the receiver {self_type} is not established from the scanned source"
        ));
    };
    if is_primitive(base) {
        return Established::Public {
            route: base.to_string(),
            conditions: Vec::new(),
        };
    }
    type_route(model, module, base)
}

/// The bare type name of a written self type, generics ignored.
fn bare_receiver(self_type: &str) -> Option<&str> {
    let base = self_type.split('<').next()?.trim();
    let first = base.chars().next()?;
    if (first.is_ascii_alphabetic() || first == '_')
        && base.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        Some(base)
    } else {
        None
    }
}

const PRIMITIVES: &[&str] = &[
    "bool", "char", "str", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64",
    "u128", "usize", "f32", "f64", "()",
];

fn is_primitive(name: &str) -> bool {
    PRIMITIVES.contains(&name)
}

/// The established route of a type name resolvable in `module`.
fn type_route(model: &CrateModel, module: &str, name: &str) -> Established {
    let mut glob_before = false;
    let mut m = Some(module.to_string());
    while let Some(path) = m {
        let def_path = qualified(&path, name);
        if glob_before
            && (model.types.contains_key(&def_path) || model.binding_in(&path, name).is_some())
        {
            return Established::Unknown(format!(
                "the receiver {name} may be shadowed by a glob import"
            ));
        }
        if let Some(def) = model.types.get(&def_path) {
            return item_route(model, &def_path, def.visible);
        }
        if let Some(binding) = model.binding_in(&path, name) {
            match resolve_use_target(model, &path, &binding.segments, binding.rooted) {
                Some(target) if model.types.contains_key(&target) => {
                    let def = model
                        .types
                        .get(&target)
                        .expect("a just-checked type definition");
                    let mut established = item_route(model, &target, def.visible);
                    if let Established::Public { conditions, .. } = &mut established {
                        conditions.extend(binding.cfg.iter().cloned());
                        conditions.extend(route_path_conditions(model, &target));
                    }
                    return established;
                }
                Some(_) => {
                    return Established::Unknown(format!(
                        "the receiver {name} binds to a type outside the scanned source"
                    ));
                }
                None => {
                    return Established::Unknown(format!(
                        "the receiver {name} binds to an unresolvable path"
                    ));
                }
            }
        }
        if model.glob_in(&path) {
            glob_before = true;
        }
        m = parent_of(&path);
    }
    Established::Unknown(format!(
        "the receiver {name} is not defined in the scanned source"
    ))
}

/// The resolved target of a written use path, `None` when unresolvable.
fn resolve_use_target(
    model: &CrateModel,
    from: &str,
    written: &[String],
    rooted: bool,
) -> Option<String> {
    resolve_use_target_inner(model, from, written, rooted, 0)
}

fn resolve_use_target_inner(
    model: &CrateModel,
    from: &str,
    written: &[String],
    rooted: bool,
    hops: usize,
) -> Option<String> {
    let mut queue: VecDeque<String> = written.iter().cloned().collect();
    let mut rooted = rooted;
    if !rooted && queue.front().is_some_and(|first| first == "crate") {
        queue.pop_front();
        rooted = true;
    }
    let mut base: Vec<String> = if rooted {
        Vec::new()
    } else {
        segments_of(from).into_iter().map(String::from).collect()
    };
    let mut redirects = hops;
    while let Some(segment) = queue.pop_front() {
        match segment.as_str() {
            "self" => {}
            "super" => {
                if base.is_empty() {
                    return None;
                }
                base.pop();
            }
            name => {
                let base_path = join_path(base.iter().map(String::as_str));
                let mut candidate = base.clone();
                candidate.push(name.to_string());
                if model
                    .modules
                    .contains_key(&join_path(candidate.iter().map(String::as_str)))
                {
                    base = candidate;
                    continue;
                }
                let binding = model.binding_in(&base_path, name).or_else(|| {
                    (base_path == from)
                        .then(|| model.binding_ancestor(&base_path, name))
                        .flatten()
                });
                let Some(binding) = binding else {
                    if queue.is_empty() {
                        base = candidate;
                        return Some(join_path(base.iter().map(String::as_str)));
                    }
                    return None;
                };
                redirects += 1;
                if redirects > 8 {
                    return None;
                }
                let target = resolve_use_target_inner(
                    model,
                    &base_path,
                    &binding.segments,
                    binding.rooted,
                    redirects,
                )?;
                if model.modules.contains_key(&target) {
                    base = segments_of(&target).into_iter().map(String::from).collect();
                } else if queue.is_empty() {
                    return Some(target);
                } else {
                    return None;
                }
            }
        }
    }
    Some(join_path(base.iter().map(String::as_str)))
}

/// The cumulative module paths below `path`, the item name excluded.
fn module_segments(path: &str) -> Vec<String> {
    let segments = segments_of(path);
    let modules = segments.len().saturating_sub(1);
    (1..=modules)
        .map(|end| segments[..end].join("::"))
        .collect()
}

/// `path` split on `::`, empty segments dropped.
fn segments_of(path: &str) -> Vec<&str> {
    path.split("::").filter(|s| !s.is_empty()).collect()
}

/// The parent of `path`, `None` for a top module.
fn parent_of(path: &str) -> Option<String> {
    let segments = segments_of(path);
    let last = segments.len().checked_sub(1)?;
    Some(segments[..last].join("::"))
}

/// `tail` appended to `module`, the bare name at the crate root.
fn qualified(module: &str, tail: &str) -> String {
    if module.is_empty() {
        tail.to_string()
    } else {
        format!("{module}::{tail}")
    }
}

/// The last `::` segment of `path`.
fn last_segment(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path)
}

/// `segments` joined on `::`, empty entries dropped.
fn join_path<I>(segments: I) -> String
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut out = String::new();
    for segment in segments {
        let segment = segment.as_ref();
        if segment.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("::");
        }
        out.push_str(segment);
    }
    out
}

/// `route` qualified with the target name.
fn qualify(name: &str, route: String) -> String {
    join_path([name.to_string(), route])
}

/// `path` as a workspace-relative site path with forward slashes and no leading `./` or `/`.
fn workspace_relative(path: &str, root: &str) -> String {
    let stripped = path.strip_prefix(root).unwrap_or(path);
    let mut out = stripped.replace('\\', "/");
    while let Some(rest) = out.strip_prefix("./") {
        out = rest.to_string();
    }
    while let Some(rest) = out.strip_prefix('/') {
        out = rest.to_string();
    }
    out
}

/// The directory of `path`, `""` at the top.
fn dir_of(path: &str) -> String {
    match path.rfind('/') {
        Some(index) => path[..index].to_string(),
        None => String::new(),
    }
}

/// The file stem of `path`, its last extension dropped.
fn stem_of(path: &str) -> &str {
    let file = last_segment(path);
    match file.rfind('.') {
        Some(index) if index > 0 => &file[..index],
        _ => file,
    }
}

/// `dir` and `name` joined, `.` dropped and `..` taking off the previous component.
fn lexical_join(dir: &str, name: &str) -> String {
    let name = name.replace('\\', "/");
    if name.starts_with('/') {
        return name;
    }
    let mut out: Vec<&str> = dir
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    for component in name.split('/').filter(|c| !c.is_empty()) {
        match component {
            "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out.join("/")
}

/// `dir/name` joined on one separator.
fn join_dir(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// The callback span's positions.
type SpanKey = (u32, u32, u32, u32);

fn span_key(span: Span) -> SpanKey {
    let start = span.start();
    let end = span.end();
    (
        u32::try_from(start.line).unwrap_or(u32::MAX),
        u32::try_from(start.column).unwrap_or(u32::MAX),
        u32::try_from(end.line).unwrap_or(u32::MAX),
        u32::try_from(end.column).unwrap_or(u32::MAX),
    )
}

/// The first-to-last token span of `node`, the way the context emitter measures it.
fn node_span(node: &impl ToTokens) -> SpanKey {
    let mut first = None;
    let mut last = None;
    for tree in node.to_token_stream() {
        if first.is_none() {
            first = Some(tree.span());
        }
        last = Some(tree.span());
    }
    let last = last.expect("a recorded node carries tokens");
    let first = first.unwrap_or(last);
    span_key(first.join(last).unwrap_or_else(Span::call_site))
}

/// The inline module chain, enclosing cfgs and function flavour of each emitted context.
struct Meta {
    chain: Vec<String>,
    cfg: Vec<String>,
    flavor: Flavor,
}

/// What the context is, from the source walk.
enum Flavor {
    /// A context embedded in an item, requiring extraction.
    Embedded,
    /// A whole free function body.
    Free { visible: Vis, nested: bool },
    /// A whole method body, `self_type` as written on the impl.
    Method {
        self_type: String,
        visible: Vis,
        nested: bool,
    },
    /// A whole trait default body.
    TraitDefault {
        trait_name: String,
        trait_visible: Vis,
        nested: bool,
    },
}

/// The declared visibility of an item.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Vis {
    /// No `pub`.
    Inherited,
    /// A bare `pub`.
    Public,
    /// A restricted `pub(crate)`, `pub(super)` or `pub(path)`.
    Restricted,
}

fn vis_class(vis: &syn::Visibility) -> Vis {
    match vis {
        syn::Visibility::Inherited => Vis::Inherited,
        syn::Visibility::Public(_) => Vis::Public,
        syn::Visibility::Restricted(_) => Vis::Restricted,
    }
}

/// The walk that records one meta per emitted context node.
struct MetaWalk {
    chain: Vec<String>,
    cfg: Vec<String>,
    fns: usize,
    owner: Option<Owner>,
    out: BTreeMap<SpanKey, Meta>,
}

/// The enclosing impl self type or trait, for method and trait-default flavours.
#[derive(Clone)]
struct Owner {
    /// The impl self type as written.
    self_type: String,
    /// The enclosing trait name.
    trait_name: Option<String>,
    /// The enclosing trait's declared visibility.
    trait_vis: Vis,
}

impl MetaWalk {
    fn record(&mut self, key: SpanKey, flavor: Flavor) {
        self.out.insert(
            key,
            Meta {
                chain: self.chain.clone(),
                cfg: self.cfg.clone(),
                flavor,
            },
        );
    }
}

impl Visit<'_> for MetaWalk {
    fn visit_item_mod(&mut self, item: &syn::ItemMod) {
        let depth = self.cfg.len();
        self.cfg.extend(cfgs(&item.attrs));
        self.chain.push(item.ident.to_string());
        syn::visit::visit_item_mod(self, item);
        self.chain.pop();
        self.cfg.truncate(depth);
    }

    fn visit_item_fn(&mut self, item: &syn::ItemFn) {
        let depth = self.cfg.len();
        self.cfg.extend(cfgs(&item.attrs));
        self.record(
            node_span(item.block.as_ref()),
            Flavor::Free {
                visible: vis_class(&item.vis),
                nested: self.fns > 0,
            },
        );
        self.fns += 1;
        syn::visit::visit_item_fn(self, item);
        self.fns -= 1;
        self.cfg.truncate(depth);
    }

    fn visit_item_impl(&mut self, item: &syn::ItemImpl) {
        let depth = self.cfg.len();
        self.cfg.extend(cfgs(&item.attrs));
        let owner = Owner {
            self_type: type_text(&item.self_ty),
            trait_name: None,
            trait_vis: Vis::Inherited,
        };
        let previous = self.owner.replace(owner);
        syn::visit::visit_item_impl(self, item);
        self.owner = previous;
        self.cfg.truncate(depth);
    }

    fn visit_item_trait(&mut self, item: &syn::ItemTrait) {
        let depth = self.cfg.len();
        self.cfg.extend(cfgs(&item.attrs));
        let owner = Owner {
            self_type: String::new(),
            trait_name: Some(item.ident.to_string()),
            trait_vis: vis_class(&item.vis),
        };
        let previous = self.owner.replace(owner);
        syn::visit::visit_item_trait(self, item);
        self.owner = previous;
        self.cfg.truncate(depth);
    }

    fn visit_item_const(&mut self, item: &syn::ItemConst) {
        let depth = self.cfg.len();
        self.cfg.extend(cfgs(&item.attrs));
        syn::visit::visit_item_const(self, item);
        self.cfg.truncate(depth);
    }

    fn visit_item_static(&mut self, item: &syn::ItemStatic) {
        let depth = self.cfg.len();
        self.cfg.extend(cfgs(&item.attrs));
        syn::visit::visit_item_static(self, item);
        self.cfg.truncate(depth);
    }

    fn visit_impl_item(&mut self, item: &syn::ImplItem) {
        let syn::ImplItem::Fn(f) = item else {
            syn::visit::visit_impl_item(self, item);
            return;
        };
        let depth = self.cfg.len();
        self.cfg.extend(cfgs(&f.attrs));
        let owner = self.owner.clone().expect("an enclosing impl or trait");
        self.record(
            node_span(&f.block),
            Flavor::Method {
                self_type: owner.self_type,
                visible: vis_class(&f.vis),
                nested: self.fns > 0,
            },
        );
        self.fns += 1;
        syn::visit::visit_impl_item(self, item);
        self.fns -= 1;
        self.cfg.truncate(depth);
    }

    fn visit_trait_item(&mut self, item: &syn::TraitItem) {
        let syn::TraitItem::Fn(f) = item else {
            syn::visit::visit_trait_item(self, item);
            return;
        };
        let Some(block) = f.default.as_ref() else {
            syn::visit::visit_trait_item(self, item);
            return;
        };
        let depth = self.cfg.len();
        self.cfg.extend(cfgs(&f.attrs));
        let owner = self.owner.clone().expect("an enclosing impl or trait");
        self.record(
            node_span(block),
            Flavor::TraitDefault {
                trait_name: owner.trait_name.unwrap_or_default(),
                trait_visible: owner.trait_vis,
                nested: self.fns > 0,
            },
        );
        self.fns += 1;
        syn::visit::visit_trait_item(self, item);
        self.fns -= 1;
        self.cfg.truncate(depth);
    }

    fn visit_expr_if(&mut self, expr: &syn::ExprIf) {
        self.record(node_span(&expr.then_branch), Flavor::Embedded);
        syn::visit::visit_expr_if(self, expr);
    }

    fn visit_expr_for_loop(&mut self, expr: &syn::ExprForLoop) {
        self.record(node_span(&expr.body), Flavor::Embedded);
        syn::visit::visit_expr_for_loop(self, expr);
    }

    fn visit_expr_while(&mut self, expr: &syn::ExprWhile) {
        self.record(node_span(&expr.body), Flavor::Embedded);
        syn::visit::visit_expr_while(self, expr);
    }

    fn visit_expr_loop(&mut self, expr: &syn::ExprLoop) {
        self.record(node_span(&expr.body), Flavor::Embedded);
        syn::visit::visit_expr_loop(self, expr);
    }

    fn visit_expr_block(&mut self, expr: &syn::ExprBlock) {
        self.record(node_span(expr), Flavor::Embedded);
        syn::visit::visit_expr_block(self, expr);
    }

    fn visit_expr_unsafe(&mut self, expr: &syn::ExprUnsafe) {
        self.record(node_span(expr), Flavor::Embedded);
        syn::visit::visit_expr_unsafe(self, expr);
    }

    fn visit_expr_async(&mut self, expr: &syn::ExprAsync) {
        self.record(node_span(expr), Flavor::Embedded);
        syn::visit::visit_expr_async(self, expr);
    }

    fn visit_expr_try_block(&mut self, expr: &syn::ExprTryBlock) {
        self.record(node_span(expr), Flavor::Embedded);
        syn::visit::visit_expr_try_block(self, expr);
    }

    fn visit_expr_const(&mut self, expr: &syn::ExprConst) {
        self.record(node_span(expr), Flavor::Embedded);
        syn::visit::visit_expr_const(self, expr);
    }

    fn visit_arm(&mut self, arm: &syn::Arm) {
        self.record(node_span(arm), Flavor::Embedded);
        syn::visit::visit_arm(self, arm);
    }

    fn visit_expr_closure(&mut self, closure: &syn::ExprClosure) {
        self.record(node_span(closure), Flavor::Embedded);
        syn::visit::visit_expr_closure(self, closure);
    }
}

fn context_meta(parsed: &syn::File) -> BTreeMap<SpanKey, Meta> {
    let mut walk = MetaWalk {
        chain: Vec::new(),
        cfg: Vec::new(),
        fns: 0,
        owner: None,
        out: BTreeMap::new(),
    };
    walk.visit_file(parsed);
    walk.out
}

/// The printed `cfg` attributes among `attrs`.
fn cfgs(attrs: &[syn::Attribute]) -> impl Iterator<Item = String> + '_ {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("cfg"))
        .map(|a| a.to_token_stream().to_string())
}

/// The type as written, `Foo<u8>` and `a::Foo` kept whole.
fn type_text(ty: &syn::Type) -> String {
    let printed = ty.to_token_stream().to_string();
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let mut out = String::with_capacity(printed.len());
    let mut chars = printed.chars().peekable();
    while let Some(c) = chars.next() {
        if c != ' ' || (word(out.chars().last()) && word(chars.peek().copied())) {
            out.push(c);
        }
    }
    out
}

/// The crate model with modules, type definitions and import bindings by module path.
struct CrateModel {
    /// Modules by target-relative path.
    modules: BTreeMap<String, ModuleInfo>,
    /// Type definitions by target-relative path.
    types: BTreeMap<String, TypeDef>,
    /// Import bindings by module path, source order per module.
    bindings: BTreeMap<String, Vec<Binding>>,
    /// Whether every externally declared module resolved to a scanned file.
    resolved: bool,
}

struct ModuleInfo {
    /// The module's declared `pub`.
    visible: bool,
    /// An external declaration whose file the target carries.
    resolved: bool,
    /// The declaration's cfg attributes.
    cfg: Vec<String>,
    /// The alternatives of the module's available claims, none while unconditional.
    claims: Vec<Vec<String>>,
}

struct TypeDef {
    /// The type's declared `pub`.
    visible: bool,
    /// The definition's cfg attributes.
    cfg: Vec<String>,
}

#[derive(Clone)]
struct Binding {
    /// The alias name, `None` for a glob.
    alias: Option<String>,
    /// The written target segments.
    segments: Vec<String>,
    /// A leading `::` or `crate::`.
    rooted: bool,
    /// A bare `pub use`.
    visible: bool,
    /// The use declaration's cfg attributes.
    cfg: Vec<String>,
}

impl CrateModel {
    fn new(target: &TargetScan, layout: &CrateLayout) -> Self {
        let mut modules = BTreeMap::new();
        let file_set: BTreeSet<String> = target
            .files
            .iter()
            .map(|f| workspace_relative(&f.path, ""))
            .collect();
        let mut types = BTreeMap::new();
        let mut bindings: BTreeMap<String, Vec<Binding>> = BTreeMap::new();
        let mut resolved = true;
        for file in &target.files {
            let path = workspace_relative(&file.path, "");
            let file_decls = layout.decls.get(&path).expect("a drained file");
            let memberships = layout.memberships.get(&path).expect("a layout file");
            let items = item_defs(&file.parsed);
            for membership in memberships {
                for decl in file_decls {
                    let module = join_path(membership.module.iter().chain(decl.module.iter()));
                    match modules.entry(module) {
                        alloc::collections::btree_map::Entry::Occupied(mut entry) => {
                            let info: &mut ModuleInfo = entry.get_mut();
                            info.visible &= decl.visible;
                            info.resolved &= decl.resolved;
                        }
                        alloc::collections::btree_map::Entry::Vacant(entry) => {
                            entry.insert(ModuleInfo {
                                visible: decl.visible,
                                resolved: decl.resolved,
                                cfg: decl.cfg.clone(),
                                claims: claim_alternatives(decl, &file_set),
                            });
                        }
                    }
                    if !decl.inline && !decl.resolved {
                        resolved = false;
                    }
                }
                for (chain, def) in &items.defs {
                    let path = join_path(
                        membership
                            .module
                            .iter()
                            .chain(chain.iter())
                            .chain(core::iter::once(&def.name)),
                    );
                    types.entry(path).or_insert_with(|| TypeDef {
                        visible: def.visible,
                        cfg: def.cfg.clone(),
                    });
                }
                for (chain, binding) in &items.bindings {
                    let module = join_path(membership.module.iter().chain(chain.iter()));
                    bindings.entry(module).or_default().push(binding.clone());
                }
            }
        }
        CrateModel {
            modules,
            types,
            bindings,
            resolved,
        }
    }

    fn binding_in(&self, module: &str, name: &str) -> Option<&Binding> {
        self.bindings
            .get(module)
            .and_then(|list| list.iter().find(|b| b.alias.as_deref() == Some(name)))
    }

    /// The first declared module whose file the target does not carry.
    fn first_unresolved_module(&self) -> Option<String> {
        self.modules
            .iter()
            .find(|(_, info)| !info.resolved)
            .map(|(module, _)| module.clone())
    }

    fn binding_ancestor(&self, module: &str, name: &str) -> Option<&Binding> {
        let mut m = parent_of(module);
        while let Some(path) = m {
            if let Some(binding) = self.binding_in(&path, name) {
                return Some(binding);
            }
            m = parent_of(&path);
        }
        None
    }

    fn glob_in(&self, module: &str) -> bool {
        self.bindings
            .get(module)
            .is_some_and(|list| list.iter().any(|b| b.alias.is_none()))
    }

    /// All modules of `path` exist and are visible.
    fn chain_visible(&self, path: &str) -> bool {
        module_segments(path).iter().all(|module| {
            self.modules
                .get(module)
                .is_some_and(|i| i.visible && i.resolved)
        })
    }

    /// All modules of `path` exist, visibility unknowns included.
    fn chain_known(&self, path: &str) -> bool {
        module_segments(path)
            .iter()
            .all(|module| self.modules.contains_key(module))
    }

    /// All modules of `module` exist and are visible, the module itself included.
    fn module_chain_visible(&self, module: &str) -> bool {
        let segments = segments_of(module);
        (1..=segments.len()).all(|end| {
            self.modules
                .get(&segments[..end].join("::"))
                .is_some_and(|info| info.visible && info.resolved)
        })
    }
}

/// The alternatives of a declaration's available claim units, none while every
/// unit resolves or an available unit is unconditional.
fn claim_alternatives(decl: &Decl, file_set: &BTreeSet<String>) -> Vec<Vec<String>> {
    let mut units: Vec<Vec<String>> = Vec::new();
    for (pred, _, _) in &decl.paths {
        if !units.iter().any(|unit| unit == pred) {
            units.push(pred.clone());
        }
    }
    let available: Vec<Vec<String>> = units
        .iter()
        .filter(|unit| {
            decl.paths
                .iter()
                .any(|(pred, path, _)| pred == *unit && file_set.contains(path))
        })
        .cloned()
        .collect();
    if available.len() == units.len() || available.iter().any(alloc::vec::Vec::is_empty) {
        return Vec::new();
    }
    available
}

/// An externally declared module and where it resolves.
struct Decl {
    /// The inline chain below the file, plus the module name.
    module: Vec<String>,
    /// A declared bare `pub`.
    visible: bool,
    /// An inline module.
    inline: bool,
    /// The declaration resolves to a file the target carries.
    resolved: bool,
    /// The declaration's cfg attributes.
    cfg: Vec<String>,
    /// The claim file paths, their branch conditions and mod-rs flags.
    paths: Vec<(Vec<String>, String, bool)>,
}

/// The module path of one file membership and the alternatives of its claim conditions.
struct Membership {
    /// The module path segments.
    module: Vec<String>,
    /// The claim alternatives under which the file is linked.
    alternatives: Vec<Vec<String>>,
}

/// The file layout with declarations, module memberships and claim contexts.
struct CrateLayout {
    /// The module declarations of every file, keyed by file path.
    decls: BTreeMap<String, Vec<Decl>>,
    /// The module memberships and branch conditions of each file, the discovered segments first.
    memberships: BTreeMap<String, Vec<Membership>>,
}

/// The directory context a file's module declarations resolve against.
#[derive(Clone)]
struct FileClaim {
    /// The directory of implicit child files.
    base: String,
    /// The directory of `#[path]` child files.
    dir: String,
}

fn crate_layout(target: &TargetScan) -> CrateLayout {
    let file_set: BTreeSet<String> = target
        .files
        .iter()
        .map(|f| workspace_relative(&f.path, ""))
        .collect();
    let file_index: BTreeMap<String, usize> = target
        .files
        .iter()
        .enumerate()
        .map(|(index, f)| (workspace_relative(&f.path, ""), index))
        .collect();
    let mut decls = BTreeMap::new();
    let mut memberships: BTreeMap<String, Vec<Membership>> = BTreeMap::new();
    let mut contexts: BTreeMap<String, FileClaim> = BTreeMap::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for file in &target.files {
        let path = workspace_relative(&file.path, "");
        if !memberships.contains_key(&path) {
            memberships.insert(
                path.clone(),
                alloc::vec![Membership {
                    module: file.segments.clone(),
                    alternatives: Vec::new(),
                }],
            );
        }
        if file.segments.is_empty() {
            let dir = dir_of(&path);
            if contexts
                .insert(
                    path.clone(),
                    FileClaim {
                        base: dir.clone(),
                        dir,
                    },
                )
                .is_none()
            {
                queue.push_back(path);
            }
        }
    }
    drain_queue(
        target,
        &file_set,
        &file_index,
        &mut decls,
        &mut memberships,
        &mut contexts,
        &mut queue,
    );
    let unclaimed: Vec<String> = memberships
        .keys()
        .filter(|path| !decls.contains_key(*path))
        .cloned()
        .collect();
    for path in unclaimed {
        let index = *file_index.get(&path).expect("a layout file");
        let file = &target.files[index];
        let dir = dir_of(&path);
        let claim = if file.segments.is_empty() || path.ends_with("/mod.rs") {
            FileClaim {
                base: dir.clone(),
                dir: dir.clone(),
            }
        } else {
            FileClaim {
                base: join_dir(&dir, stem_of(&path)),
                dir: dir.clone(),
            }
        };
        if contexts.insert(path.clone(), claim).is_none() {
            queue.push_back(path);
        }
    }
    drain_queue(
        target,
        &file_set,
        &file_index,
        &mut decls,
        &mut memberships,
        &mut contexts,
        &mut queue,
    );
    CrateLayout { decls, memberships }
}

/// Process queued files by collecting their declarations and following their claims.
fn drain_queue(
    target: &TargetScan,
    file_set: &BTreeSet<String>,
    file_index: &BTreeMap<String, usize>,
    decls: &mut BTreeMap<String, Vec<Decl>>,
    memberships: &mut BTreeMap<String, Vec<Membership>>,
    contexts: &mut BTreeMap<String, FileClaim>,
    queue: &mut VecDeque<String>,
) {
    while let Some(path) = queue.pop_front() {
        let index = *file_index.get(&path).expect("an enqueued file");
        let file = &target.files[index];
        let claim = contexts.get(&path).expect("an enqueued context").clone();
        let mut walk = DeclWalk {
            chain: Vec::new(),
            base: alloc::vec![claim.base.clone()],
            dir: alloc::vec![claim.dir.clone()],
            pred: alloc::vec![Vec::new()],
            file_set,
            out: Vec::new(),
        };
        walk.visit_file(&file.parsed);
        let own = walk.out;
        for decl in &own {
            if decl.inline {
                continue;
            }
            for (branch, claim_path, mod_rs) in &decl.paths {
                if !file_set.contains(claim_path) {
                    continue;
                }
                let entry = memberships.entry(claim_path.clone()).or_default();
                match entry.iter_mut().find(|m| m.module == decl.module) {
                    Some(existing) => {
                        if !existing.alternatives.contains(branch) {
                            existing.alternatives.push(branch.clone());
                        }
                    }
                    None => entry.push(Membership {
                        module: decl.module.clone(),
                        alternatives: alloc::vec![branch.clone()],
                    }),
                }
                if contexts.contains_key(claim_path) {
                    continue;
                }
                let dir = dir_of(claim_path);
                contexts.insert(
                    claim_path.clone(),
                    FileClaim {
                        base: if *mod_rs {
                            dir.clone()
                        } else {
                            join_dir(&dir, stem_of(claim_path))
                        },
                        dir,
                    },
                );
                queue.push_back(claim_path.clone());
            }
        }
        decls.insert(path, own);
    }
}

/// The `mod` declarations of one parsed file, with their claim paths.
struct DeclWalk<'a> {
    chain: Vec<String>,
    base: Vec<String>,
    dir: Vec<String>,
    /// The accumulated condition of each context, parallel to `base` and `dir`.
    pred: Vec<Vec<String>>,
    file_set: &'a BTreeSet<String>,
    out: Vec<Decl>,
}

impl<'a> Visit<'a> for DeclWalk<'a> {
    fn visit_item_mod(&mut self, item: &'a syn::ItemMod) {
        let name = item.ident.unraw().to_string();
        let depth = self.chain.len();
        self.chain.push(item.ident.to_string());
        if let Some((_, items)) = &item.content {
            self.inline_mod(item, &name, items, depth);
        } else {
            self.file_mod(item, &name, depth);
        }
    }
}

impl<'a> DeclWalk<'a> {
    fn inline_mod(
        &mut self,
        item: &'a syn::ItemMod,
        name: &str,
        items: &'a [syn::Item],
        depth: usize,
    ) {
        self.out.push(Decl {
            module: self.chain.clone(),
            visible: matches!(item.vis, syn::Visibility::Public(_)),
            inline: true,
            resolved: true,
            cfg: cfgs(&item.attrs).collect(),
            paths: Vec::new(),
        });
        let claimed = path_claims(&item.attrs, &claim_atom);
        let mut inner_base: Vec<String> = Vec::new();
        let mut inner_dir: Vec<String> = Vec::new();
        let mut inner_pred: Vec<Vec<String>> = Vec::new();
        let mut push_context = |context: String, pred: Vec<String>| {
            if !inner_dir.contains(&context) {
                inner_base.push(context.clone());
                inner_dir.push(context);
                inner_pred.push(pred);
            }
        };
        for ((base, dir), outer) in self.base.iter().zip(&self.dir).zip(&self.pred) {
            let context = if let Some(path) = &claimed.direct {
                lexical_join(dir, path)
            } else {
                join_dir(base, name)
            };
            let dir_pred = if claimed.direct.is_some() {
                &claimed.direct_pred
            } else {
                &claimed.fallback
            };
            push_context(
                context,
                outer
                    .iter()
                    .cloned()
                    .chain(dir_pred.iter().cloned())
                    .collect(),
            );
            for (branch, path) in &claimed.branches {
                push_context(
                    lexical_join(dir, path),
                    outer
                        .iter()
                        .cloned()
                        .chain(branch.iter().cloned())
                        .collect(),
                );
            }
        }
        let (previous_base, previous_dir, previous_pred) =
            (self.base.clone(), self.dir.clone(), self.pred.clone());
        self.base = inner_base;
        self.dir = inner_dir;
        self.pred = inner_pred;
        for child in items {
            syn::visit::visit_item(self, child);
        }
        self.base = previous_base;
        self.dir = previous_dir;
        self.pred = previous_pred;
        self.chain.truncate(depth);
    }

    fn file_mod(&mut self, item: &'a syn::ItemMod, name: &str, depth: usize) {
        let cfg: Vec<String> = cfgs(&item.attrs).collect();
        let claimed = path_claims(&item.attrs, &claim_atom);
        let mut claims: Vec<(Vec<String>, String, bool)> = Vec::new();
        let mut seen: BTreeSet<(Vec<String>, String, bool)> = BTreeSet::new();
        let add = |claims: &mut Vec<(Vec<String>, String, bool)>,
                   seen: &mut BTreeSet<(Vec<String>, String, bool)>,
                   pred: Vec<String>,
                   claim: String,
                   mod_rs: bool| {
            if seen.insert((pred.clone(), claim.clone(), mod_rs)) {
                claims.push((pred, claim, mod_rs));
            }
        };
        for ((base, dir), outer) in self.base.iter().zip(&self.dir).zip(&self.pred) {
            let join =
                |pred: &[String]| outer.iter().cloned().chain(pred.iter().cloned()).collect();
            if let Some(path) = &claimed.direct {
                add(
                    &mut claims,
                    &mut seen,
                    join(&claimed.direct_pred),
                    lexical_join(dir, path),
                    true,
                );
            } else {
                add(
                    &mut claims,
                    &mut seen,
                    join(&claimed.fallback),
                    join_dir(base, &format!("{name}.rs")),
                    false,
                );
                add(
                    &mut claims,
                    &mut seen,
                    join(&claimed.fallback),
                    join_dir(base, &format!("{name}/mod.rs")),
                    true,
                );
            }
            for (pred, path) in &claimed.branches {
                add(
                    &mut claims,
                    &mut seen,
                    join(pred),
                    lexical_join(dir, path),
                    true,
                );
            }
        }
        self.out.push(Decl {
            module: self.chain.clone(),
            visible: matches!(item.vis, syn::Visibility::Public(_)),
            inline: false,
            resolved: claims
                .iter()
                .any(|(_, path, _)| self.file_set.contains(path)),
            cfg,
            paths: claims,
        });
        self.chain.truncate(depth);
    }
}

/// One `cfg_attr` predicate as a context condition.
fn claim_atom(pred: &syn::Meta) -> String {
    format!("cfg({})", pred.to_token_stream())
}

/// Type definitions and import bindings at every module depth of one file.
struct ItemDefs {
    defs: Vec<(Vec<String>, ItemDef)>,
    bindings: Vec<(Vec<String>, Binding)>,
}

struct ItemDef {
    name: String,
    visible: bool,
    cfg: Vec<String>,
}

/// The walk that records type definitions and import bindings.
struct DefWalk {
    chain: Vec<String>,
    fns: usize,
    exprs: usize,
    out: ItemDefs,
}

impl DefWalk {
    fn module_scope(&self) -> bool {
        self.fns == 0 && self.exprs == 0
    }

    fn def_item(&mut self, name: &syn::Ident, visible: bool, attrs: &[syn::Attribute]) {
        if self.module_scope() {
            self.out.defs.push((
                self.chain.clone(),
                ItemDef {
                    name: name.to_string(),
                    visible,
                    cfg: cfgs(attrs).collect(),
                },
            ));
        }
    }
}

impl Visit<'_> for DefWalk {
    fn visit_item_mod(&mut self, item: &syn::ItemMod) {
        let depth = self.chain.len();
        self.chain.push(item.ident.to_string());
        syn::visit::visit_item_mod(self, item);
        self.chain.truncate(depth);
    }

    fn visit_item_fn(&mut self, item: &syn::ItemFn) {
        self.fns += 1;
        syn::visit::visit_item_fn(self, item);
        self.fns -= 1;
    }

    fn visit_impl_item(&mut self, item: &syn::ImplItem) {
        if matches!(item, syn::ImplItem::Fn(_)) {
            self.fns += 1;
        }
        syn::visit::visit_impl_item(self, item);
        if matches!(item, syn::ImplItem::Fn(_)) {
            self.fns -= 1;
        }
    }

    fn visit_trait_item(&mut self, item: &syn::TraitItem) {
        if matches!(item, syn::TraitItem::Fn(f) if f.default.is_some()) {
            self.fns += 1;
        }
        syn::visit::visit_trait_item(self, item);
        if matches!(item, syn::TraitItem::Fn(f) if f.default.is_some()) {
            self.fns -= 1;
        }
    }

    fn visit_expr_block(&mut self, expr: &syn::ExprBlock) {
        if !self.module_scope() {
            return;
        }
        self.exprs += 1;
        syn::visit::visit_expr_block(self, expr);
        self.exprs -= 1;
    }

    fn visit_expr_closure(&mut self, expr: &syn::ExprClosure) {
        if !self.module_scope() {
            return;
        }
        self.exprs += 1;
        syn::visit::visit_expr_closure(self, expr);
        self.exprs -= 1;
    }

    fn visit_expr_if(&mut self, expr: &syn::ExprIf) {
        if !self.module_scope() {
            return;
        }
        self.exprs += 1;
        syn::visit::visit_expr_if(self, expr);
        self.exprs -= 1;
    }

    fn visit_expr_for_loop(&mut self, expr: &syn::ExprForLoop) {
        if !self.module_scope() {
            return;
        }
        self.exprs += 1;
        syn::visit::visit_expr_for_loop(self, expr);
        self.exprs -= 1;
    }

    fn visit_expr_while(&mut self, expr: &syn::ExprWhile) {
        if !self.module_scope() {
            return;
        }
        self.exprs += 1;
        syn::visit::visit_expr_while(self, expr);
        self.exprs -= 1;
    }

    fn visit_expr_loop(&mut self, expr: &syn::ExprLoop) {
        if !self.module_scope() {
            return;
        }
        self.exprs += 1;
        syn::visit::visit_expr_loop(self, expr);
        self.exprs -= 1;
    }

    fn visit_expr_unsafe(&mut self, expr: &syn::ExprUnsafe) {
        if !self.module_scope() {
            return;
        }
        self.exprs += 1;
        syn::visit::visit_expr_unsafe(self, expr);
        self.exprs -= 1;
    }

    fn visit_expr_async(&mut self, expr: &syn::ExprAsync) {
        if !self.module_scope() {
            return;
        }
        self.exprs += 1;
        syn::visit::visit_expr_async(self, expr);
        self.exprs -= 1;
    }

    fn visit_expr_try_block(&mut self, expr: &syn::ExprTryBlock) {
        if !self.module_scope() {
            return;
        }
        self.exprs += 1;
        syn::visit::visit_expr_try_block(self, expr);
        self.exprs -= 1;
    }

    fn visit_expr_const(&mut self, expr: &syn::ExprConst) {
        if !self.module_scope() {
            return;
        }
        self.exprs += 1;
        syn::visit::visit_expr_const(self, expr);
        self.exprs -= 1;
    }

    fn visit_arm(&mut self, arm: &syn::Arm) {
        if !self.module_scope() {
            return;
        }
        self.exprs += 1;
        syn::visit::visit_arm(self, arm);
        self.exprs -= 1;
    }

    fn visit_item_struct(&mut self, item: &syn::ItemStruct) {
        self.def_item(
            &item.ident,
            matches!(item.vis, syn::Visibility::Public(_)),
            &item.attrs,
        );
        syn::visit::visit_item_struct(self, item);
    }

    fn visit_item_enum(&mut self, item: &syn::ItemEnum) {
        self.def_item(
            &item.ident,
            matches!(item.vis, syn::Visibility::Public(_)),
            &item.attrs,
        );
        syn::visit::visit_item_enum(self, item);
    }

    fn visit_item_union(&mut self, item: &syn::ItemUnion) {
        self.def_item(
            &item.ident,
            matches!(item.vis, syn::Visibility::Public(_)),
            &item.attrs,
        );
        syn::visit::visit_item_union(self, item);
    }

    fn visit_item_type(&mut self, item: &syn::ItemType) {
        self.def_item(
            &item.ident,
            matches!(item.vis, syn::Visibility::Public(_)),
            &item.attrs,
        );
        syn::visit::visit_item_type(self, item);
    }

    fn visit_item_use(&mut self, item: &syn::ItemUse) {
        if self.module_scope() {
            let visible = matches!(item.vis, syn::Visibility::Public(_));
            let cfg: Vec<String> = cfgs(&item.attrs).collect();
            for binding in use_bindings(item) {
                self.out.bindings.push((
                    self.chain.clone(),
                    Binding {
                        visible,
                        cfg: cfg.clone(),
                        ..binding
                    },
                ));
            }
        }
        // Imports are bound by their enclosing scope and the walk stops here.
    }
}

fn item_defs(parsed: &syn::File) -> ItemDefs {
    let mut walk = DefWalk {
        chain: Vec::new(),
        fns: 0,
        exprs: 0,
        out: ItemDefs {
            defs: Vec::new(),
            bindings: Vec::new(),
        },
    };
    walk.visit_file(parsed);
    walk.out
}

/// The leaf bindings of one `use` tree.
fn use_bindings(item: &syn::ItemUse) -> Vec<Binding> {
    let mut out = Vec::new();
    use_leaf(item.leading_colon.is_some(), &[], &item.tree, &mut out);
    out
}

fn use_leaf(rooted: bool, prefix: &[String], tree: &syn::UseTree, out: &mut Vec<Binding>) {
    match tree {
        syn::UseTree::Path(path) => {
            let mut prefix = prefix.to_vec();
            prefix.push(path.ident.to_string());
            use_leaf(rooted, &prefix, &path.tree, out);
        }
        syn::UseTree::Name(name) => {
            let mut segments = prefix.to_vec();
            segments.push(name.ident.to_string());
            out.push(Binding {
                alias: Some(name.ident.to_string()),
                segments,
                rooted,
                visible: false,
                cfg: Vec::new(),
            });
        }
        syn::UseTree::Rename(rename) => {
            let mut segments = prefix.to_vec();
            segments.push(rename.ident.to_string());
            out.push(Binding {
                alias: Some(rename.rename.to_string()),
                segments,
                rooted,
                visible: false,
                cfg: Vec::new(),
            });
        }
        syn::UseTree::Glob(_) => {
            out.push(Binding {
                alias: None,
                segments: prefix.to_vec(),
                rooted,
                visible: false,
                cfg: Vec::new(),
            });
        }
        syn::UseTree::Group(group) => {
            for tree in &group.items {
                use_leaf(rooted, prefix, tree, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_scope::ContextScope;
    use crate::{ContextKind, SourceFile, TargetScan};
    use alloc::vec;

    fn file(path: &str, segments: &[&str], code: &str) -> SourceFile {
        SourceFile {
            path: path.to_string(),
            segments: segments.iter().map(ToString::to_string).collect(),
            cfg: Vec::new(),
            parsed: syn::parse_file(code).unwrap(),
            text: code.to_string(),
            rustdoc: true,
        }
    }

    fn target(files: &[SourceFile], library: bool) -> TargetScan {
        TargetScan {
            name: "app".into(),
            library,
            files: files.to_vec(),
        }
    }

    fn records(code: &str) -> Vec<IndexedContext> {
        let file = file("src/lib.rs", &[], code);
        extract(&target(&[file], true), "", false, &[])
    }

    fn one<'a>(record: &'a [IndexedContext], module: &str) -> &'a IndexedContext {
        record
            .iter()
            .find(|r| r.module == module)
            .unwrap_or_else(|| {
                panic!(
                    "no record for {module}: {:?}",
                    record.iter().map(|r| r.module.as_str()).collect::<Vec<_>>()
                )
            })
    }

    #[test]
    fn records_carry_form_tokens_module_and_site() {
        let out = records("pub fn helper(x: u32) -> u32 { x + 1u32 }");
        assert_eq!(out.len(), 1);
        let record = &out[0];
        assert_eq!(record.site.file, "src/lib.rs");
        assert_eq!(record.site.item, "app::helper");
        assert_eq!(record.module, "app");
        assert_eq!(record.crate_name, "app");
        assert_eq!(record.crate_root, "src/lib.rs");
        assert_eq!(record.access, ContextAccess::PublicFunction);
        assert_eq!(record.api_path.as_deref(), Some("app::helper"));
        assert_eq!(record.conditions, Vec::<String>::new());
        assert_eq!(record.extraction_notes, Vec::<String>::new());
    }

    #[test]
    fn private_module_function_is_internal() {
        let out = records("mod inner { pub fn helper(x: u32) -> u32 { x + 1u32 } }");
        let record = one(&out, "app::inner");
        assert_eq!(record.access, ContextAccess::InternalFunction);
        assert_eq!(record.api_path, None);
    }

    #[test]
    fn pub_reexport_restores_a_public_route() {
        let out = records(
            "mod inner { pub fn helper(x: u32) -> u32 { x + 1u32 } }\npub use inner::helper;",
        );
        let record = one(&out, "app::inner");
        assert_eq!(record.access, ContextAccess::PublicFunction);
        assert_eq!(record.api_path.as_deref(), Some("app::helper"));
        // The original site item keeps the private-module path as evidence.
        assert_eq!(record.site.item, "app::inner::helper");
    }

    #[test]
    fn reexport_on_a_private_chain_stays_internal() {
        let out = records(
            "mod inner { pub fn helper(x: u32) -> u32 { x + 1u32 } }\nmod api { use super::inner::helper; }",
        );
        let record = one(&out, "app::inner");
        assert_eq!(record.access, ContextAccess::InternalFunction);
    }

    #[test]
    fn pub_reexport_in_a_private_module_stays_internal() {
        let out = records(
            "mod inner { pub fn helper(x: u32) -> u32 { x + 1u32 } }\nmod api { pub use super::inner::helper; }",
        );
        let record = one(&out, "app::inner");
        assert_eq!(record.access, ContextAccess::InternalFunction);
        assert_eq!(record.api_path, None);
        assert_eq!(record.conditions, Vec::<String>::new());
    }

    #[test]
    fn reexport_through_a_public_module_restores_the_route() {
        let out = records(
            "mod inner { pub fn helper(x: u32) -> u32 { x + 1u32 } }\npub mod api { pub use super::inner::helper; }",
        );
        let record = one(&out, "app::inner");
        assert_eq!(record.access, ContextAccess::PublicFunction);
        assert_eq!(record.api_path.as_deref(), Some("app::api::helper"));
    }

    #[test]
    fn module_reexports_route_the_items_inside() {
        let inner = "mod inner { pub mod deep { pub mod sub { pub fn helper(x: u32) -> u32 { x + 1u32 } } } mod hidden { pub fn secret(x: u32) -> u32 { x + 2u32 } } }";
        for (uses, module, route) in [
            (
                "pub use inner::deep;",
                "app::inner::deep::sub",
                Some("app::deep::sub::helper"),
            ),
            (
                "pub use inner::deep as alias;",
                "app::inner::deep::sub",
                Some("app::alias::sub::helper"),
            ),
            (
                "pub use inner::deep::sub;",
                "app::inner::deep::sub",
                Some("app::sub::helper"),
            ),
            (
                "pub use inner::*;",
                "app::inner::deep::sub",
                Some("app::deep::sub::helper"),
            ),
            (
                "pub use inner::deep::*;",
                "app::inner::deep::sub",
                Some("app::sub::helper"),
            ),
            ("pub use inner::*;", "app::inner::hidden", None),
            ("pub use inner::deep;", "app::inner::hidden", None),
        ] {
            let out = records(&format!("{inner}\n{uses}"));
            let record = one(&out, module);
            assert_eq!(record.api_path.as_deref(), route, "{uses} {module}");
            let access = if route.is_some() {
                ContextAccess::PublicFunction
            } else {
                ContextAccess::InternalFunction
            };
            assert_eq!(record.access, access, "{uses} {module}");
        }
    }

    #[test]
    fn a_method_on_a_type_in_a_reexported_module_is_public() {
        let out = records(
            "mod inner { pub mod deep { pub struct Pack; impl Pack { pub fn size(&self) -> u32 { 1u32 } } } }\npub use inner::deep;",
        );
        let record = one(&out, "app::inner::deep");
        assert_eq!(record.access, ContextAccess::PublicMethod);
        assert_eq!(record.api_path.as_deref(), Some("app::deep::Pack::size"));
    }

    #[test]
    fn a_module_public_under_one_cfg_only_is_not_public() {
        let out = records(
            "#[cfg(unix)]\npub mod m { pub fn helper(x: u32) -> u32 { x + 1u32 } }\n#[cfg(not(unix))]\nmod m { pub fn helper(x: u32) -> u32 { x + 2u32 } }",
        );
        for record in out.iter().filter(|record| record.module == "app::m") {
            assert_ne!(record.access, ContextAccess::PublicFunction, "{record:?}");
            assert_eq!(record.api_path, None, "{record:?}");
        }
    }

    #[test]
    fn restricted_pub_is_not_an_external_route() {
        let out = records("pub(crate) fn helper(x: u32) -> u32 { x + 1u32 }");
        let record = &out[0];
        assert_eq!(record.access, ContextAccess::InternalFunction);
        assert_eq!(record.api_path, None);
    }

    #[test]
    fn method_on_a_public_type_is_public() {
        let out = records("pub struct Box;\nimpl Box { pub fn pack(&self) -> u32 { 1u32 } }");
        let record = &out[0];
        assert_eq!(record.access, ContextAccess::PublicMethod);
        assert_eq!(record.api_path.as_deref(), Some("app::Box::pack"));
    }

    #[test]
    fn method_on_an_internal_type_is_internal() {
        let out = records("struct Box;\nimpl Box { pub fn pack(&self) -> u32 { 1u32 } }");
        let record = &out[0];
        assert_eq!(record.access, ContextAccess::InternalMethod);
        assert!(
            record
                .extraction_notes
                .iter()
                .any(|note| note.contains("internal to the crate"))
        );
    }

    #[test]
    fn method_on_a_foreign_type_is_unresolved() {
        let out = records("impl External { pub fn pack(&self) -> u32 { 1u32 } }");
        let record = &out[0];
        assert_eq!(record.access, ContextAccess::Unresolved);
        assert!(
            record
                .extraction_notes
                .iter()
                .any(|note| note.contains("External"))
        );
    }

    #[test]
    fn method_receiver_resolves_across_files_through_an_import() {
        let files = vec![
            file("src/lib.rs", &[], "pub struct Box;\npub mod store;"),
            file(
                "src/store.rs",
                &["store"],
                "use crate::Box;\nimpl Box { pub fn pack(&self) -> u32 { 1u32 } }",
            ),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let record = one(&out, "app::store");
        assert_eq!(record.access, ContextAccess::PublicMethod);
        assert_eq!(record.api_path.as_deref(), Some("app::Box::pack"));
    }

    #[test]
    fn trait_defaults_follow_the_trait_visibility() {
        let pub_trait = records("pub trait Seal { fn stamp(&self) -> u32 { 1u32 } }");
        assert_eq!(one(&pub_trait, "app").access, ContextAccess::PublicMethod);
        assert_eq!(
            one(&pub_trait, "app").api_path.as_deref(),
            Some("app::Seal::stamp")
        );
        let private_trait = records("trait Seal { fn stamp(&self) -> u32 { 1u32 } }");
        assert_eq!(
            one(&private_trait, "app").access,
            ContextAccess::InternalMethod
        );
        let reexported = records(
            "mod inner { pub trait Seal { fn stamp(&self) -> u32 { 1u32 } } }\npub use inner::Seal;",
        );
        let record = one(&reexported, "app::inner");
        assert_eq!(record.access, ContextAccess::PublicMethod);
        assert_eq!(record.api_path.as_deref(), Some("app::Seal::stamp"));
    }

    #[test]
    fn nested_functions_are_internal() {
        let out = records(
            "pub fn outer(x: u32) -> u32 { fn inner(y: u32) -> u32 { y + 1u32 } inner(x) }",
        );
        let inner = out
            .iter()
            .find(|r| r.site.item == "app::outer::inner")
            .unwrap();
        let outer = out.iter().find(|r| r.site.item == "app::outer").unwrap();
        assert_eq!(outer.access, ContextAccess::PublicFunction);
        assert_eq!(inner.access, ContextAccess::InternalFunction);
        assert!(
            inner
                .extraction_notes
                .iter()
                .any(|note| note.contains("nested"))
        );
    }

    #[test]
    fn bin_targets_are_unresolved() {
        let file = file(
            "src/main.rs",
            &[],
            "pub fn helper(x: u32) -> u32 { x + 1u32 }",
        );
        let out = extract(&target(&[file], false), "", false, &[]);
        assert_eq!(out[0].access, ContextAccess::Unresolved);
        assert!(
            out[0]
                .extraction_notes
                .iter()
                .any(|note| note.contains("not a library"))
        );
    }

    #[test]
    fn unresolvable_module_keeps_access_unresolved() {
        let files = vec![
            file("src/lib.rs", &[], "pub mod leaf;\npub mod missing;"),
            file(
                "src/leaf.rs",
                &["leaf"],
                "fn helper(x: u32) -> u32 { x + 1u32 }",
            ),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let record = one(&out, "app::leaf");
        assert_eq!(record.access, ContextAccess::Unresolved);
        assert!(
            record
                .extraction_notes
                .iter()
                .any(|note| note.contains("missing"))
        );
    }

    #[test]
    fn resolved_private_function_ignores_unrelated_missing_modules() {
        let files = vec![
            file(
                "src/lib.rs",
                &[],
                "pub mod leaf;\npub mod missing;\npub fn helper(x: u32) -> u32 { x + 1u32 }",
            ),
            file("src/leaf.rs", &["leaf"], ""),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let helper = out.iter().find(|r| r.site.item == "app::helper").unwrap();
        assert_eq!(helper.access, ContextAccess::PublicFunction);
    }

    #[test]
    fn cfg_conditions_retain_the_enclosing_branches() {
        let out = records(
            "#[cfg(unix)]\nmod inner {\n    #[cfg(feature = \"extra\")]\n    pub fn helper(x: u32) -> u32 { x + 1u32 }\n}",
        );
        let record = one(&out, "app::inner");
        assert_eq!(record.conditions.len(), 2);
        assert!(record.conditions.iter().any(|c| c.contains("unix")));
        assert!(record.conditions.iter().any(|c| c.contains("feature")));
    }

    #[test]
    fn cfg_attr_branches_condition_the_public_routes() {
        let files = vec![
            file(
                "src/lib.rs",
                &[],
                "#[cfg_attr(feature = \"alternate\", path = \"alternate.rs\")]\npub mod m;\npub fn helper(x: u32) -> u32 { x + 1u32 }\n",
            ),
            file(
                "src/m.rs",
                &["m"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }\n",
            ),
            file(
                "src/alternate.rs",
                &["m"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }\n",
            ),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let default = out
            .iter()
            .find(|r| r.site.file == "src/m.rs" && r.module == "app::m")
            .unwrap();
        let branch = out
            .iter()
            .find(|r| r.site.file == "src/alternate.rs" && r.module == "app::m")
            .unwrap();
        assert_eq!(default.access, ContextAccess::PublicFunction);
        assert_eq!(branch.access, ContextAccess::PublicFunction);
        assert_eq!(default.api_path.as_deref(), Some("app::m::helper"));
        assert_eq!(branch.api_path.as_deref(), Some("app::m::helper"));
        assert_eq!(default.conditions, ["not(cfg(feature = \"alternate\"))"]);
        assert_eq!(branch.conditions, ["cfg(feature = \"alternate\")"]);
    }

    #[test]
    fn a_direct_path_before_a_branch_wins_every_configuration() {
        let files = vec![
            file(
                "src/lib.rs",
                &[],
                "#[path = \"base.rs\"]\n#[cfg_attr(feature = \"alternate\", path = \"alternate.rs\")]\npub mod m;\n",
            ),
            file(
                "src/base.rs",
                &["m"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }\n",
            ),
            file(
                "src/alternate.rs",
                &["m"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }\n",
            ),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let base = out
            .iter()
            .find(|r| r.site.file == "src/base.rs" && r.module == "app::m")
            .unwrap();
        let branch = out
            .iter()
            .find(|r| r.site.file == "src/alternate.rs" && r.module == "app::m")
            .unwrap();
        assert_eq!(base.access, ContextAccess::PublicFunction);
        assert_eq!(branch.access, ContextAccess::Unresolved);
        assert_eq!(base.api_path.as_deref(), Some("app::m::helper"));
        assert_eq!(branch.api_path, None);
        assert_eq!(base.conditions, Vec::<String>::new());
        assert_eq!(
            branch.conditions,
            ["all(cfg(feature = \"alternate\"), false)"]
        );
        assert!(
            branch
                .extraction_notes
                .iter()
                .any(|note| note.contains("shadowed"))
        );
    }

    #[test]
    fn earlier_branches_exclude_later_ones_selecting_the_same_file() {
        let files = vec![
            file(
                "src/lib.rs",
                &[],
                "#[cfg_attr(feature = \"a\", path = \"x.rs\")]\n#[cfg_attr(feature = \"b\", path = \"x.rs\")]\npub mod m;\n",
            ),
            file(
                "src/m.rs",
                &["m"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }\n",
            ),
            file(
                "src/x.rs",
                &["m"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }\n",
            ),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let default = out
            .iter()
            .find(|r| r.site.file == "src/m.rs" && r.module == "app::m")
            .unwrap();
        let branch = out
            .iter()
            .find(|r| r.site.file == "src/x.rs" && r.module == "app::m")
            .unwrap();
        assert_eq!(
            default.conditions,
            ["all(not(cfg(feature = \"a\")), not(cfg(feature = \"b\")))"]
        );
        assert_eq!(
            branch.conditions,
            ["any(cfg(feature = \"a\"), all(cfg(feature = \"b\"), not(cfg(feature = \"a\"))))"]
        );
    }

    #[test]
    fn inline_module_children_claim_every_branch_directory() {
        let files = vec![
            file(
                "src/lib.rs",
                &[],
                "#[cfg_attr(feature = \"alt\", path = \"alt/m.rs\")]\npub mod m {\n    pub mod child;\n}\n",
            ),
            file(
                "src/m/child.rs",
                &["m", "child"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }\n",
            ),
            file(
                "src/alt/child.rs",
                &["m", "child"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }\n",
            ),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let site_files: Vec<&str> = out.iter().map(|r| r.site.file.as_str()).collect();
        assert!(site_files.contains(&"src/m/child.rs"));
        assert!(site_files.contains(&"src/alt/child.rs"));
        assert_eq!(
            out.iter().filter(|r| r.module == "app::m::child").count(),
            2
        );
    }

    #[test]
    fn a_reexport_route_carries_the_branch_conditions_of_its_module() {
        let files = vec![
            file(
                "src/lib.rs",
                &[],
                "#[cfg_attr(feature = \"alt\", path = \"alt_api.rs\")]\nmod api;\npub use api::helper;\n",
            ),
            file(
                "src/api.rs",
                &["api"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }\n",
            ),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let record = one(&out, "app::api");
        assert_eq!(record.access, ContextAccess::PublicFunction);
        assert_eq!(record.api_path.as_deref(), Some("app::helper"));
        assert_eq!(record.conditions, ["not(cfg(feature = \"alt\"))"]);
    }

    #[test]
    fn embedded_contexts_are_classified_embedded() {
        let out = records(
            "pub fn outer(x: u32) -> u32 {\n    let body = { x + 1u32 };\n    let pick = |y: u32| y + 1u32;\n    let arm = match x { _ => { x + 1u32 } };\n    arm + pick(body) + body\n}",
        );
        let outer = out.iter().find(|r| r.site.item == "app::outer").unwrap();
        assert_eq!(outer.access, ContextAccess::PublicFunction);
        assert_eq!(outer.api_path.as_deref(), Some("app::outer"));
        let embedded = out
            .iter()
            .filter(|r| r.access == ContextAccess::Embedded)
            .count();
        assert_eq!(embedded, 4);
        assert!(out.iter().all(|r| r.module == "app"));
        assert!(
            out.iter()
                .filter(|r| r.site.kind != ContextKind::FunctionBody)
                .all(|r| r.access == ContextAccess::Embedded)
        );
    }

    #[test]
    fn module_level_const_block_is_embedded() {
        let out = records("const SUM: u32 = { 1u32 + 1u32 };");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].access, ContextAccess::Embedded);
        assert_eq!(out[0].site.item, "app::SUM");
    }

    #[test]
    fn extract_keeps_sub_floor_records() {
        let out = records("fn tiny() -> u32 { 1u32 }");
        assert_eq!(out.len(), 1);
        assert!(out[0].tokens < 30);
    }

    #[test]
    fn aliased_module_memberships_select_and_route() {
        let files = vec![
            file(
                "src/lib.rs",
                &[],
                "mod a;\n#[path = \"a.rs\"]\npub mod b;\npub use b::helper;",
            ),
            file(
                "src/a.rs",
                &["a"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }",
            ),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        assert_eq!(out.len(), 2);
        let by_a = one(&out, "app::a");
        let by_b = one(&out, "app::b");
        assert_eq!(by_a.access, ContextAccess::InternalFunction);
        assert_eq!(by_b.access, ContextAccess::PublicFunction);
        assert_eq!(by_b.api_path.as_deref(), Some("app::b::helper"));
        assert_eq!(by_a.site.file, by_b.site.file);
        assert_eq!(by_a.site.line, by_b.site.line);
        let exact_a = ContextScope::ModuleExact("app::a".to_string());
        let exact_b = ContextScope::ModuleExact("app::b".to_string());
        assert!(out.iter().any(|r| exact_a.matches(r)));
        assert!(
            !out.iter()
                .any(|r| ContextScope::ModuleExact("app::c".to_string()).matches(r))
        );
        assert!(out.iter().any(|r| exact_b.matches(r)));
    }

    #[test]
    fn generated_files_are_skipped() {
        let files = vec![file(
            "src/lib.rs",
            &[],
            "// @generated\npub fn helper(x: u32) -> u32 { x + 1u32 }",
        )];
        let skipped = extract(&target(&files, true), "", false, &[]);
        assert_eq!(skipped, Vec::<IndexedContext>::new());
        let scanned = extract(&target(&files, true), "", true, &[]);
        assert_eq!(scanned.len(), 1);
    }

    #[test]
    fn shared_file_records_keep_their_crate() {
        let file = file(
            "src/shared.rs",
            &[],
            "pub fn helper(x: u32) -> u32 { x + 1u32 }",
        );
        let first_target = TargetScan {
            name: "one".into(),
            library: true,
            files: vec![file.clone()],
        };
        let two = TargetScan {
            name: "two".into(),
            library: true,
            files: vec![file],
        };
        let first = extract(&first_target, "", false, &[]);
        let second = extract(&two, "", false, &[]);
        assert_eq!(first[0].crate_name, "one");
        assert_eq!(second[0].crate_name, "two");
        assert_eq!(first[0].site.file, second[0].site.file);
        assert_eq!(first[0].site.line, second[0].site.line);
        assert_eq!(first[0].site.column, second[0].site.column);
        assert_eq!(first[0].site.end, second[0].site.end);
        assert_eq!(first[0].site.end_column, second[0].site.end_column);
        assert_eq!(first[0].site.kind, second[0].site.kind);
        assert_eq!(first[0].site.item, "one::helper");
        assert_eq!(second[0].site.item, "two::helper");
    }
    #[test]
    fn nested_cfg_attr_module_paths_resolve_and_report_their_condition_chain() {
        let files = vec![
            file(
                "src/lib.rs",
                &[],
                "#[cfg_attr(unix, cfg_attr(feature = \"alt\", path = \"alt_leaf.rs\"))] pub mod leaf;\npub fn root_fn(x: u32) -> u32 { x * 2u32 }",
            ),
            file(
                "src/alt_leaf.rs",
                &["leaf"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }",
            ),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let record = one(&out, "app::leaf");
        assert_eq!(record.site.file, "src/alt_leaf.rs");
        assert_eq!(record.access, ContextAccess::PublicFunction);
        assert_eq!(record.api_path.as_deref(), Some("app::leaf::helper"));
        assert_eq!(
            record.conditions,
            vec!["all(cfg(unix), cfg(feature = \"alt\"))"]
        );
    }

    #[test]
    fn nested_cfg_attr_inline_directories_resolve_and_report_their_condition_chain() {
        let files = vec![
            file(
                "src/lib.rs",
                &[],
                "#[cfg_attr(unix, cfg_attr(feature = \"alt\", path = \"altbox\"))] pub mod boxy { pub mod child; }",
            ),
            file(
                "src/altbox/child.rs",
                &["boxy", "child"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }",
            ),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let record = one(&out, "app::boxy::child");
        assert_eq!(record.site.file, "src/altbox/child.rs");
        assert_eq!(record.access, ContextAccess::PublicFunction);
        assert_eq!(record.api_path.as_deref(), Some("app::boxy::child::helper"));
        assert_eq!(
            record.conditions,
            vec!["all(cfg(unix), cfg(feature = \"alt\"))"]
        );
    }
    #[test]
    fn a_branch_after_a_direct_path_is_never_linked() {
        let files = vec![
            file(
                "src/lib.rs",
                &[],
                "#[path = \"main_leaf.rs\"]\n#[cfg_attr(unix, path = \"alt_leaf.rs\")]\npub mod leaf;\npub fn root_fn(x: u32) -> u32 { x * 2u32 }",
            ),
            file(
                "src/main_leaf.rs",
                &["leaf"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }",
            ),
            file(
                "src/alt_leaf.rs",
                &["leaf"],
                "pub fn helper(x: u32) -> u32 { x + 7u32 }",
            ),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let main = out
            .iter()
            .find(|r| r.site.file == "src/main_leaf.rs")
            .unwrap();
        let alt = out
            .iter()
            .find(|r| r.site.file == "src/alt_leaf.rs")
            .unwrap();
        assert_eq!(main.access, ContextAccess::PublicFunction);
        assert_eq!(main.api_path.as_deref(), Some("app::leaf::helper"));
        assert_eq!(main.conditions, Vec::<String>::new());
        assert_eq!(alt.access, ContextAccess::Unresolved);
        assert_eq!(alt.api_path, None);
        assert_eq!(alt.conditions, vec!["all(cfg(unix), false)"]);
        assert!(
            alt.extraction_notes
                .iter()
                .any(|note| note.contains("shadowed"))
        );
    }

    #[test]
    fn a_branch_before_a_direct_path_shadows_it_only_when_active() {
        let files = vec![
            file(
                "src/lib.rs",
                &[],
                "#[cfg_attr(unix, path = \"alt_leaf.rs\")]\n#[path = \"main_leaf.rs\"]\npub mod leaf;\npub fn root_fn(x: u32) -> u32 { x * 2u32 }",
            ),
            file(
                "src/main_leaf.rs",
                &["leaf"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }",
            ),
            file(
                "src/alt_leaf.rs",
                &["leaf"],
                "pub fn helper(x: u32) -> u32 { x + 7u32 }",
            ),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let main = out
            .iter()
            .find(|r| r.site.file == "src/main_leaf.rs")
            .unwrap();
        let alt = out
            .iter()
            .find(|r| r.site.file == "src/alt_leaf.rs")
            .unwrap();
        assert_eq!(main.conditions, vec!["not(cfg(unix))"]);
        assert_eq!(alt.conditions, vec!["cfg(unix)"]);
    }
    #[test]
    fn a_bin_method_and_trait_default_are_not_established() {
        let out = extract(
            &target(
                &[file(
                    "src/main.rs",
                    &[],
                    "struct A; impl A { pub fn m(&self) -> u32 { 1u32 } } trait T { fn d(&self) -> u32 { 1u32 } }",
                )],
                false,
            ),
            "",
            false,
            &[],
        );
        let method = out.iter().find(|r| r.site.item == "app::A::m").unwrap();
        let default = out.iter().find(|r| r.site.item == "app::T::d").unwrap();
        for record in [method, default] {
            assert_eq!(
                record.access,
                ContextAccess::Unresolved,
                "{}",
                record.site.item
            );
            assert!(
                record
                    .extraction_notes
                    .iter()
                    .any(|note| note.contains("not a library"))
            );
        }
    }

    #[test]
    fn a_root_qualified_self_type_keeps_its_leading_colons_in_the_item() {
        let out = extract(
            &target(
                &[file(
                    "src/lib.rs",
                    &[],
                    "pub trait T { fn m(&self) -> u32; } impl T for ::core::cell::Cell<u32> { fn m(&self) -> u32 { 1u32 } }",
                )],
                true,
            ),
            "",
            false,
            &[],
        );
        let items: Vec<&str> = out.iter().map(|r| r.site.item.as_str()).collect();
        assert!(
            items.contains(&"app:::: core :: cell :: Cell < u32 >::m"),
            "{items:?}"
        );
    }

    #[test]
    fn items_inside_a_function_body_are_nested() {
        let out = records(
            "struct A;\npub fn outer() -> u32 { impl A { pub fn m(&self) -> u32 { 1u32 } } 1u32 }",
        );
        let nested = out
            .iter()
            .find(|r| r.site.item == "app::outer::A::m")
            .unwrap();
        assert_eq!(nested.access, ContextAccess::InternalMethod);
        assert!(
            nested
                .extraction_notes
                .iter()
                .any(|note| note.contains("nested"))
        );
    }

    #[test]
    fn a_trait_inside_a_function_body_is_nested() {
        let out = records("pub fn outer() -> u32 { trait T { fn d(&self) -> u32 { 1u32 } } 1u32 }");
        let nested = out
            .iter()
            .find(|r| r.site.item == "app::outer::T::d")
            .unwrap();
        assert_eq!(nested.access, ContextAccess::InternalMethod);
        assert!(
            nested
                .extraction_notes
                .iter()
                .any(|note| note.contains("nested"))
        );
    }

    #[test]
    fn a_private_method_stays_internal() {
        let out = records("pub struct A;\nimpl A { fn m(&self) -> u32 { 1u32 } }");
        let record = one(&out, "app");
        assert_eq!(record.access, ContextAccess::InternalMethod);
        assert_eq!(record.api_path, None);
    }

    #[test]
    fn a_trait_default_on_an_undeclared_chain_is_unresolved() {
        let out = extract(
            &target(
                &[file(
                    "src/b.rs",
                    &["a", "b"],
                    "pub trait T { fn d(&self) -> u32 { 1u32 } }",
                )],
                true,
            ),
            "",
            false,
            &[],
        );
        let record = one(&out, "app::a::b");
        assert_eq!(record.access, ContextAccess::Unresolved);
        assert!(
            record
                .extraction_notes
                .iter()
                .any(|note| note.contains("module a is not found"))
        );
    }

    #[test]
    fn a_reference_receiver_is_not_established() {
        let out = records("struct A;\nimpl &A { pub fn m(&self) -> u32 { 1u32 } }");
        let record = one(&out, "app");
        assert_eq!(record.access, ContextAccess::Unresolved);
        assert!(
            record
                .extraction_notes
                .iter()
                .any(|note| note.contains("&A"))
        );
    }

    #[test]
    fn a_primitive_receiver_is_public() {
        let out = records("impl u32 { pub fn inc(&self) -> u32 { *self + 1u32 } }");
        let record = one(&out, "app");
        assert_eq!(record.access, ContextAccess::PublicMethod);
        assert_eq!(record.api_path.as_deref(), Some("app::u32::inc"));
    }

    #[test]
    fn a_type_shadowed_by_a_glob_import_is_unresolved() {
        let out = records(
            "pub struct Pack;\nmod inner { use super::*; impl Pack { pub fn m(&self) -> u32 { 1u32 } } }",
        );
        let record = one(&out, "app::inner");
        assert_eq!(record.access, ContextAccess::Unresolved);
        assert!(
            record
                .extraction_notes
                .iter()
                .any(|note| note.contains("shadowed by a glob import"))
        );
    }

    #[test]
    fn a_receiver_aliased_to_a_module_is_unresolved() {
        let out = records(
            "mod inner { pub struct Pack; }\nmod m { use crate::inner as Pack; impl Pack { pub fn m(&self) -> u32 { 1u32 } } }",
        );
        let record = one(&out, "app::m");
        assert_eq!(record.access, ContextAccess::Unresolved);
        assert!(
            record
                .extraction_notes
                .iter()
                .any(|note| note.contains("outside the scanned source"))
        );
    }

    #[test]
    fn a_receiver_bound_to_an_unresolvable_path_is_unresolved() {
        let out =
            records("mod m { use nope::Pack; impl Pack { pub fn m(&self) -> u32 { 1u32 } } }");
        let record = one(&out, "app::m");
        assert_eq!(record.access, ContextAccess::Unresolved);
        assert!(
            record
                .extraction_notes
                .iter()
                .any(|note| note.contains("unresolvable path"))
        );
    }

    #[test]
    fn an_undeclared_receiver_is_not_defined() {
        let out = records("mod m { impl Ghost { pub fn m(&self) -> u32 { 1u32 } } }");
        let record = one(&out, "app::m");
        assert_eq!(record.access, ContextAccess::Unresolved);
        assert!(
            record
                .extraction_notes
                .iter()
                .any(|note| note.contains("not defined in the scanned source"))
        );
    }

    #[test]
    fn a_terminating_redirect_cycle_yields_no_reexport() {
        let out = records(
            "mod x { pub struct T; }\nmod deep { pub fn helper(x: u32) -> u32 { x + 1u32 } }\npub mod api { use crate::x::T as P; pub use crate::api::b as a; pub use crate::api::a as b; pub use P::c; }",
        );
        let record = one(&out, "app::deep");
        assert_eq!(record.access, ContextAccess::InternalFunction);
        assert_eq!(record.api_path, None);
    }

    #[test]
    fn a_route_through_an_ancestor_binding_follows_the_target_module() {
        let files = vec![file(
            "src/lib.rs",
            &[],
            "mod store { pub fn helper(x: u32) -> u32 { x + 1u32 } pub fn other(x: u32) -> u32 { x + 2u32 } }\nuse crate::store as P;\npub mod n { pub use P::helper; pub use Q::missing; }",
        )];
        let out = extract(&target(&files, true), "", false, &[]);
        let helper = out
            .iter()
            .find(|r| r.site.item == "app::store::helper")
            .unwrap();
        let other = out
            .iter()
            .find(|r| r.site.item == "app::store::other")
            .unwrap();
        assert_eq!(helper.access, ContextAccess::PublicFunction);
        assert_eq!(helper.api_path.as_deref(), Some("app::n::helper"));
        assert_eq!(other.access, ContextAccess::InternalFunction);
        assert_eq!(other.api_path, None);
    }

    #[test]
    fn a_receiver_follows_a_rebound_alias_to_the_original_type() {
        let out = records(
            "pub struct Box;\nuse crate::Box as B;\nmod n { use B as C; impl C { pub fn pack(&self) -> u32 { 1u32 } } }",
        );
        let record = one(&out, "app::n");
        assert_eq!(record.access, ContextAccess::PublicMethod);
        assert_eq!(record.api_path.as_deref(), Some("app::Box::pack"));
    }

    #[test]
    fn a_super_qualified_reexport_at_the_root_is_unresolvable() {
        let out = records(
            "mod inner { pub fn helper(x: u32) -> u32 { x + 1u32 } }\npub use super::inner::helper;",
        );
        let record = one(&out, "app::inner");
        assert_eq!(record.access, ContextAccess::InternalFunction);
        assert_eq!(record.api_path, None);
    }

    #[test]
    fn a_reexport_rest_stops_at_an_undeclared_module() {
        let files = vec![
            file("src/lib.rs", &[], "pub mod a;\npub use a::name as leaf;"),
            file("src/a.rs", &["a"], ""),
            file(
                "src/a/name/sub.rs",
                &["a", "name", "sub"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }",
            ),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let record = one(&out, "app::a::name::sub");
        assert_eq!(record.access, ContextAccess::Unresolved);
        assert!(
            record
                .extraction_notes
                .iter()
                .any(|note| note.contains("module a::name is not found"))
        );
    }

    #[test]
    fn a_dot_prefixed_root_file_is_normalized() {
        let files = vec![
            file(
                "./lib.rs",
                &[],
                "pub mod m;\npub fn helper(x: u32) -> u32 { x + 1u32 }",
            ),
            file("m.rs", &["m"], "pub fn other(x: u32) -> u32 { x + 2u32 }"),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let helper = out.iter().find(|r| r.site.item == "app::helper").unwrap();
        let other = out.iter().find(|r| r.site.item == "app::m::other").unwrap();
        assert_eq!(helper.site.file, "lib.rs");
        assert_eq!(helper.crate_root, "lib.rs");
        assert_eq!(helper.api_path.as_deref(), Some("app::helper"));
        assert_eq!(other.site.file, "m.rs");
        assert_eq!(other.api_path.as_deref(), Some("app::m::other"));
    }

    #[test]
    fn an_unclaimed_file_without_an_extension_keeps_its_context() {
        let out = extract(
            &target(
                &[file(
                    "m",
                    &["m"],
                    "pub fn helper(x: u32) -> u32 { x + 1u32 }",
                )],
                true,
            ),
            "",
            false,
            &[],
        );
        let record = one(&out, "app::m");
        assert_eq!(record.site.file, "m");
        assert_eq!(record.access, ContextAccess::Unresolved);
        assert!(
            record
                .extraction_notes
                .iter()
                .any(|note| note.contains("module m is not found"))
        );
    }

    #[test]
    fn an_unclaimed_mod_rs_file_keeps_its_context() {
        let out = extract(
            &target(
                &[file(
                    "src/m/mod.rs",
                    &["m"],
                    "pub fn helper(x: u32) -> u32 { x + 1u32 }",
                )],
                true,
            ),
            "",
            false,
            &[],
        );
        let record = one(&out, "app::m");
        assert_eq!(record.site.file, "src/m/mod.rs");
        assert_eq!(record.access, ContextAccess::Unresolved);
        assert!(
            record
                .extraction_notes
                .iter()
                .any(|note| note.contains("module m is not found"))
        );
    }

    #[test]
    fn an_inline_module_with_a_path_claim_links_its_children() {
        let files = vec![
            file(
                "lib.rs",
                &[],
                "#[path = \"m\"]\npub mod m { #[path = \"m/x.rs\"] pub mod child; }",
            ),
            file(
                "m/m/x.rs",
                &["m", "child"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }",
            ),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let record = one(&out, "app::m::child");
        assert_eq!(record.site.file, "m/m/x.rs");
        assert_eq!(record.api_path.as_deref(), Some("app::m::child::helper"));
    }

    #[test]
    fn a_path_claim_without_an_extension_resolves() {
        let files = vec![
            file("lib.rs", &[], "#[path = \"m\"]\npub mod m;"),
            file("m", &["m"], "pub fn helper(x: u32) -> u32 { x + 1u32 }"),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let record = one(&out, "app::m");
        assert_eq!(record.site.file, "m");
        assert_eq!(record.api_path.as_deref(), Some("app::m::helper"));
    }

    #[test]
    fn a_rooted_path_claim_keeps_its_absolute_form() {
        assert_eq!(lexical_join("src", "/abs/m.rs"), "/abs/m.rs");
        assert_eq!(lexical_join("src", "../up/m.rs"), "up/m.rs");
    }

    #[test]
    fn a_branch_onto_the_implicit_file_clears_the_module_conditions() {
        let files = vec![
            file(
                "src/lib.rs",
                &[],
                "#[cfg_attr(unix, path = \"m.rs\")]\npub mod m;",
            ),
            file(
                "src/m.rs",
                &["m"],
                "pub fn helper(x: u32) -> u32 { x + 1u32 }",
            ),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let record = one(&out, "app::m");
        assert_eq!(record.site.file, "src/m.rs");
        assert_eq!(record.access, ContextAccess::PublicFunction);
        assert_eq!(record.api_path.as_deref(), Some("app::m::helper"));
        assert_eq!(record.conditions, Vec::<String>::new());
    }

    #[test]
    fn malformed_module_attributes_do_not_block_the_implicit_claim() {
        let files = vec![
            file(
                "lib.rs",
                &[],
                "#[doc = \"x\"]\npub mod a;\n#[cfg_attr(=)]\npub mod b;\n#[cfg_attr()]\npub mod c;",
            ),
            file("a.rs", &["a"], "pub fn helper(x: u32) -> u32 { x + 1u32 }"),
            file("b.rs", &["b"], "pub fn helper(x: u32) -> u32 { x + 1u32 }"),
            file("c.rs", &["c"], "pub fn helper(x: u32) -> u32 { x + 1u32 }"),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        for module in ["app::a", "app::b", "app::c"] {
            let record = one(&out, module);
            assert_eq!(record.access, ContextAccess::PublicFunction, "{module}");
            assert_eq!(
                record.api_path.as_deref(),
                Some(format!("{module}::helper").as_str())
            );
        }
    }

    #[test]
    fn nested_cfg_attr_noise_is_skipped_around_the_path() {
        let files = vec![
            file(
                "lib.rs",
                &[],
                "#[cfg_attr(unix, doc = \"x\", cfg(target_os = \"linux\"), cfg_attr(=), cfg_attr(), path = \"alt.rs\")]\npub mod m;",
            ),
            file("m.rs", &["m"], "pub fn helper(x: u32) -> u32 { x + 1u32 }"),
            file(
                "alt.rs",
                &["m"],
                "pub fn helper(x: u32) -> u32 { x + 2u32 }",
            ),
        ];
        let out = extract(&target(&files, true), "", false, &[]);
        let default = out
            .iter()
            .find(|r| r.site.file == "m.rs" && r.module == "app::m")
            .unwrap();
        let branch = out
            .iter()
            .find(|r| r.site.file == "alt.rs" && r.module == "app::m")
            .unwrap();
        assert_eq!(default.conditions, ["not(cfg(unix))"]);
        assert_eq!(branch.conditions, ["cfg(unix)"]);
    }

    #[test]
    fn module_scope_items_define_types_and_walk_expression_bodies() {
        let out = records(
            "const PICK: fn(u32) -> u32 = |x| x;\n\
             const IF: u32 = if true { 1u32 } else { 2u32 };\n\
             const FOR: u32 = for x in 0..1u32 { x };\n\
             const WHILE: u32 = while false { 1u32 };\n\
             const LOOP: u32 = loop { 1u32 };\n\
             const UNSAFE: u32 = unsafe { 1u32 };\n\
             const ASYNC: u32 = async { 1u32 };\n\
             const TRY: u32 = try { 1u32 };\n\
             const CONST: u32 = const { 1u32 };\n\
             const MATCH: u32 = match 0u32 { _ => 1u32 };\n\
             pub enum E { V }\n\
             impl E { pub fn on(&self) -> u32 { 1u32 } }\n\
             pub union U { x: u32 }\n\
             impl U { pub fn on(&self) -> u32 { 1u32 } }\n\
             pub type Num = u32;\n\
             impl Num { pub fn inc(&self) -> u32 { *self + 1u32 } }",
        );
        let on_e = out.iter().find(|r| r.site.item == "app::E::on").unwrap();
        let on_u = out.iter().find(|r| r.site.item == "app::U::on").unwrap();
        let inc = out.iter().find(|r| r.site.item == "app::Num::inc").unwrap();
        assert_eq!(on_e.access, ContextAccess::PublicMethod);
        assert_eq!(on_e.api_path.as_deref(), Some("app::E::on"));
        assert_eq!(on_u.access, ContextAccess::PublicMethod);
        assert_eq!(on_u.api_path.as_deref(), Some("app::U::on"));
        assert_eq!(inc.access, ContextAccess::PublicMethod);
        assert_eq!(inc.api_path.as_deref(), Some("app::Num::inc"));
        assert!(
            out.iter()
                .any(|r| r.site.item == "app::IF" && r.access == ContextAccess::Embedded)
        );
    }

    #[test]
    fn loop_bodies_are_embedded_contexts() {
        let out = records(
            "pub fn spin() -> u32 { let mut n = 0u32; while n < 1u32 { n += 1u32; } loop { n += 1u32; if n >= 3u32 { break; } } n }",
        );
        let embedded = out
            .iter()
            .filter(|r| r.access == ContextAccess::Embedded)
            .count();
        assert_eq!(embedded, 3);
        let spin = out.iter().find(|r| r.site.item == "app::spin").unwrap();
        assert_eq!(spin.access, ContextAccess::PublicFunction);
    }

    #[test]
    fn special_block_expressions_are_embedded_contexts() {
        let out = records(
            "pub fn outer() -> u32 { let u = unsafe { 1u32 }; let a = async { 1u32 }; let t = try { 1u32 }; let c = const { 1u32 }; drop((a, t)); u + c }",
        );
        let embedded = out
            .iter()
            .filter(|r| r.access == ContextAccess::Embedded)
            .count();
        assert_eq!(embedded, 4);
        assert!(out.iter().all(|r| r.module == "app"));
    }

    #[test]
    fn non_function_impl_and_trait_items_are_walked_without_contexts() {
        let out = records(
            "#[cfg(unix)]\nstatic C: u32 = 0u32;\npub struct A;\nimpl A { const V: u32 = 1u32; type T = u32; }\ntrait S { type Item; fn bare(&self) -> u32; }\npub fn helper(x: u32) -> u32 { x + 1u32 }",
        );
        let helper = out.iter().find(|r| r.site.item == "app::helper").unwrap();
        assert_eq!(helper.access, ContextAccess::PublicFunction);
        assert_eq!(helper.conditions, Vec::<String>::new());
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn a_use_group_reexports_every_name() {
        let out = records(
            "mod inner { pub fn helper(x: u32) -> u32 { x + 1u32 } pub fn other(x: u32) -> u32 { x + 2u32 } }\npub use inner::{helper, other};",
        );
        let helper = out
            .iter()
            .find(|r| r.site.item == "app::inner::helper")
            .unwrap();
        let other = out
            .iter()
            .find(|r| r.site.item == "app::inner::other")
            .unwrap();
        assert_eq!(helper.api_path.as_deref(), Some("app::helper"));
        assert_eq!(other.api_path.as_deref(), Some("app::other"));
    }

    #[test]
    fn the_lookup_helper_reports_the_missing_module() {
        let out = records("pub fn helper(x: u32) -> u32 { x + 1u32 }");
        let result = std::panic::catch_unwind(|| one(&out, "app::missing"));
        assert!(result.is_err(), "the lookup of a missing module must fail");
    }
}
