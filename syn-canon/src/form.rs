//! Framed token identity and size metrics.

use alloc::string::String;
use core::fmt::Write;
use proc_macro2::{Spacing, TokenStream, TokenTree};

/// An opaque comparison key with canonical leaf and body sizes.
pub struct CanonicalForm {
    key: String,
    leaf_tokens: usize,
    body_units: usize,
}

impl CanonicalForm {
    pub(crate) fn from_tokens(tokens: TokenStream, body_units: usize) -> Self {
        let mut key = String::from("K1 S ");
        let mut leaf_tokens = 0;
        let mut scratch = String::new();
        for tree in tokens {
            framed_tree(&mut key, tree, &mut leaf_tokens, &mut scratch);
        }
        key.push('E');
        Self {
            key,
            leaf_tokens,
            body_units,
        }
    }

    /// The opaque comparison key.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The owned comparison key.
    #[must_use]
    pub fn into_key(self) -> String {
        self.key
    }

    /// Canonical leaf tokens, excluding framing.
    #[must_use]
    pub fn leaf_tokens(&self) -> usize {
        self.leaf_tokens
    }

    /// Canonical body units, excluding framing.
    #[must_use]
    pub fn body_units(&self) -> usize {
        self.body_units
    }
}

impl core::fmt::Display for CanonicalForm {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.key)
    }
}

impl core::fmt::Debug for CanonicalForm {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("CanonicalForm").field(&self.key).finish()
    }
}

impl PartialEq for CanonicalForm {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for CanonicalForm {}

impl core::hash::Hash for CanonicalForm {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        core::hash::Hash::hash(&self.key, state);
    }
}

fn framed_tree(out: &mut String, tree: TokenTree, leaves: &mut usize, scratch: &mut String) {
    match tree {
        TokenTree::Ident(ident) => {
            scratch.clear();
            write!(scratch, "{ident}").expect("writing to a string succeeds");
            framed_payload(out, 'i', scratch);
            *leaves += 1;
        }
        TokenTree::Literal(literal) => {
            scratch.clear();
            write!(scratch, "{literal}").expect("writing to a string succeeds");
            framed_payload(out, 'l', scratch);
            *leaves += 1;
        }
        TokenTree::Punct(punct) => {
            out.push('p');
            out.push(if punct.spacing() == Spacing::Joint {
                'j'
            } else {
                'a'
            });
            out.push(punct.as_char());
            out.push(' ');
            *leaves += 1;
        }
        TokenTree::Group(group) => {
            out.push('g');
            out.push(match group.delimiter() {
                proc_macro2::Delimiter::None => 'n',
                proc_macro2::Delimiter::Parenthesis => '(',
                proc_macro2::Delimiter::Bracket => '[',
                proc_macro2::Delimiter::Brace => '{',
            });
            out.push(' ');
            for tree in group.stream() {
                framed_tree(out, tree, leaves, scratch);
            }
            out.push_str("e ");
        }
    }
}

fn framed_payload(out: &mut String, kind: char, payload: &str) {
    write!(out, "{kind} {} {payload} ", payload.len()).expect("writing to a string succeeds");
}

pub(crate) fn body_units(stream: TokenStream) -> usize {
    let mut trees = stream.into_iter().peekable();
    let mut count = 0;
    let (mut ident, mut joined) = (false, false);
    while let Some(tree) = trees.next() {
        match tree {
            TokenTree::Group(group) => {
                count += body_units(group.stream());
                (ident, joined) = (false, false);
            }
            TokenTree::Ident(_) => {
                count += usize::from(!joined);
                (ident, joined) = (true, false);
            }
            TokenTree::Literal(_) => {
                count += 1;
                (ident, joined) = (false, false);
            }
            TokenTree::Punct(first) => {
                if first.as_char() == '\'' && matches!(trees.peek(), Some(TokenTree::Ident(_))) {
                    trees.next();
                    count += 1;
                    (ident, joined) = (false, false);
                    continue;
                }
                let c = first.as_char();
                let second = match trees.peek() {
                    Some(TokenTree::Punct(second)) if first.spacing() == Spacing::Joint => {
                        Some((second.as_char(), second.spacing()))
                    }
                    _ => None,
                };
                if second.is_some_and(|(second, _)| c == ':' && second == ':') {
                    trees.next();
                    joined = ident;
                    continue;
                }
                if let Some((second, spacing)) = second
                    && OPERATORS.iter().any(|op| {
                        let bytes = op.as_bytes();
                        char::from(bytes[0]) == c && char::from(bytes[1]) == second
                    })
                {
                    trees.next();
                    if spacing == Spacing::Joint
                        && let Some(TokenTree::Punct(third)) = trees.peek()
                        && OPERATORS.iter().any(|op| {
                            let bytes = op.as_bytes();
                            bytes.len() == 3
                                && char::from(bytes[0]) == c
                                && char::from(bytes[1]) == second
                                && char::from(bytes[2]) == third.as_char()
                        })
                    {
                        trees.next();
                    }
                }
                count += usize::from(c != ',');
                (ident, joined) = (false, false);
            }
        }
    }
    count
}

const OPERATORS: [&str; 23] = [
    "<<=", ">>=", "...", "..=", "=>", "==", "!=", "<=", ">=", "&&", "||", "+=", "-=", "*=", "/=",
    "%=", "^=", "&=", "|=", "<<", ">>", "..", "->",
];
