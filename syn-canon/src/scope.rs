//! Original-source binding identities and lexical namespaces.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use proc_macro2::Ident;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ns {
    Value,
    Type,
    Lifetime,
    Label,
    Macro,
}

impl Ns {
    pub(crate) fn key(self, name: &str) -> &str {
        match self {
            Self::Value | Self::Type | Self::Macro => unraw(name),
            Self::Lifetime | Self::Label => name,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct BindingId(pub(crate) usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    Parameter,
    Let,
    Closure,
    Generic,
    Lifetime,
    Label,
    Item,
    Use,
    TypeAlias,
    MacroRule,
    SelfTy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) id: BindingId,
    pub(crate) canon: String,
    pub(crate) origin: Origin,
    pub(crate) target: Option<usize>,
    pub(crate) raw: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Frame {
    names: [BTreeMap<String, Binding>; 5],
}

impl Frame {
    pub(crate) fn bind(&mut self, ns: Ns, name: &str, mut binding: Binding) {
        binding.raw = name.starts_with("r#");
        self.names[ns as usize].insert(ns.key(name).to_string(), binding);
    }

    pub(crate) fn lookup(&self, ns: Ns, name: &str) -> Option<&Binding> {
        self.names[ns as usize].get(ns.key(name))
    }

    pub(crate) fn bindings(&self, ns: Ns) -> &BTreeMap<String, Binding> {
        &self.names[ns as usize]
    }

    pub(crate) fn next_id(&self) -> usize {
        self.names
            .iter()
            .flat_map(|names| names.values())
            .map(|binding| binding.id.0)
            .max()
            .map_or(0, |id| id + 1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Domain {
    Local,
    Inherited,
}

#[derive(Clone, Debug)]
pub(crate) struct TargetSeg {
    pub(crate) name: String,
    pub(crate) origin: Option<(Domain, BindingId, Ns)>,
}

#[derive(Clone, Debug)]
pub(crate) struct Target {
    pub(crate) rooted: bool,
    pub(crate) segments: Vec<TargetSeg>,
}

pub(crate) enum FirstSeg<'a> {
    Alias(&'a Target),
    Module(Domain, BindingId),
    Free,
}

pub(crate) struct Inherited<'env> {
    pub(crate) frames: &'env [Frame],
    pub(crate) mod_frames: &'env BTreeMap<BindingId, Frame>,
    pub(crate) targets: &'env [Target],
}

pub(crate) fn unraw(name: &str) -> &str {
    name.strip_prefix("r#").unwrap_or(name)
}

pub(crate) fn ident_name(ident: &Ident) -> String {
    unraw(&ident.to_string()).to_string()
}

pub(crate) fn new_ident(canon: &str, span: proc_macro2::Span) -> Ident {
    match canon.strip_prefix("r#") {
        Some(name) => Ident::new_raw(name, span),
        None => Ident::new(canon, span),
    }
}
