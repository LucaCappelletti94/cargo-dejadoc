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

/// A fixed-width primitive scalar rustc evaluates independently of the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum PrimTy {
    /// A two's-complement integer of `width` bits.
    Int { width: u32, signed: bool },
    /// The `bool` scalar.
    Bool,
}

impl PrimTy {
    /// The canonical spelling of the suffix.
    pub(crate) fn suffix(self) -> &'static str {
        match self {
            Self::Int {
                width,
                signed: true,
            } => match width {
                8 => "i8",
                16 => "i16",
                32 => "i32",
                64 => "i64",
                128 => "i128",
                _ => unreachable!("a fixed-width primitive"),
            },
            Self::Int {
                width,
                signed: false,
            } => match width {
                8 => "u8",
                16 => "u16",
                32 => "u32",
                64 => "u64",
                128 => "u128",
                _ => unreachable!("a fixed-width primitive"),
            },
            Self::Bool => "bool",
        }
    }
}

/// The primitive a primitive name spells, raw or plain.
pub(crate) fn primitive_suffix(name: &str) -> Option<PrimTy> {
    match unraw(name) {
        "i8" => Some(PrimTy::Int {
            width: 8,
            signed: true,
        }),
        "i16" => Some(PrimTy::Int {
            width: 16,
            signed: true,
        }),
        "i32" => Some(PrimTy::Int {
            width: 32,
            signed: true,
        }),
        "i64" => Some(PrimTy::Int {
            width: 64,
            signed: true,
        }),
        "i128" => Some(PrimTy::Int {
            width: 128,
            signed: true,
        }),
        "u8" => Some(PrimTy::Int {
            width: 8,
            signed: false,
        }),
        "u16" => Some(PrimTy::Int {
            width: 16,
            signed: false,
        }),
        "u32" => Some(PrimTy::Int {
            width: 32,
            signed: false,
        }),
        "u64" => Some(PrimTy::Int {
            width: 64,
            signed: false,
        }),
        "u128" => Some(PrimTy::Int {
            width: 128,
            signed: false,
        }),
        "bool" => Some(PrimTy::Bool),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) id: BindingId,
    pub(crate) canon: String,
    pub(crate) origin: Origin,
    /// A proven fixed-width primitive scalar type.
    pub(crate) prim: Option<PrimTy>,
    pub(crate) target: Option<usize>,
    pub(crate) raw: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Frame {
    names: [BTreeMap<String, Binding>; 5],
    /// A glob import marked this scope uncertain.
    pub(crate) glob: bool,
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
            .flat_map(BTreeMap::values)
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
