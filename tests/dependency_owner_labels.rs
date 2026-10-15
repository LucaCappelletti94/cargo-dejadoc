//! Borrowed method views label their own owner by position, keeping value members qualified.
//! Owner parameters group by first use and keep their role in the owner type as a second key.

use syn_canon::{CanonicalForm, SourceContext};

/// The forms of every method named `f`, impl methods and trait defaults, in source order.
fn forms(src: &str) -> Vec<CanonicalForm> {
    let file: syn::File = syn::parse_str(src).unwrap_or_else(|e| panic!("{src}: {e}"));
    let context = SourceContext::new(core::iter::once((&[][..], &file)));
    let view = |sig: &syn::Signature, block: &syn::Block| {
        (sig.ident == "f").then(|| context.function(sig, block).unwrap().canonicalize())
    };
    file.items
        .iter()
        .flat_map(|item| match item {
            syn::Item::Impl(i) => i
                .items
                .iter()
                .filter_map(|member| match member {
                    syn::ImplItem::Fn(function) => view(&function.sig, &function.block),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            syn::Item::Trait(t) => t
                .items
                .iter()
                .filter_map(|member| match member {
                    syn::TraitItem::Fn(function) => function
                        .default
                        .as_ref()
                        .and_then(|block| view(&function.sig, block)),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        })
        .collect()
}

/// Asserts that the two `f` methods of `src` compare equal.
fn equal(src: &str) {
    let found = forms(src);
    assert_eq!(found.len(), 2, "{src}");
    assert_eq!(found[0], found[1], "{src}");
}

/// Asserts that the two `f` methods of `src` compare unequal.
fn unequal(src: &str) {
    let found = forms(src);
    assert_eq!(found.len(), 2, "{src}");
    assert_ne!(found[0], found[1], "{src}");
}

/// Whether the two `f` methods of `src` share their grouping key and their owner role key.
#[derive(Debug, PartialEq, Eq)]
struct Same {
    grouping: bool,
    roles: bool,
}

fn same(src: &str) -> Same {
    let found = forms(src);
    assert_eq!(found.len(), 2, "{src}");
    let role = |form: &CanonicalForm| form.owner_role_key().unwrap_or(form.key()).to_owned();
    Same {
        grouping: found[0] == found[1],
        roles: role(&found[0]) == role(&found[1]),
    }
}

const COPIES: Same = Same {
    grouping: true,
    roles: true,
};
const ROLE_SPLIT: Same = Same {
    grouping: true,
    roles: false,
};
const DISTINCT: Same = Same {
    grouping: false,
    roles: false,
};

#[test]
fn self_in_type_positions_matches_across_owners() {
    equal(
        "struct A; struct B;
         impl A { fn f(s: Self) -> Self { s } }
         impl B { fn f(s: Self) -> Self { s } }",
    );
    equal(
        "trait Tr { type Output; fn f(self) -> Self::Output; }
         struct A; struct B;
         impl Tr for A { type Output = u8; fn f(self) -> Self::Output { 1 } }
         impl Tr for B { type Output = u8; fn f(self) -> Self::Output { 1 } }",
    );
    equal(
        "struct A; struct B;
         impl A { fn f() -> usize { <Self as Sized>::size() } }
         impl B { fn f() -> usize { <Self as Sized>::size() } }",
    );
}

#[test]
fn self_constructors_match_across_owners() {
    equal(
        "struct A(u8); struct B(u8);
         impl A { fn f(v: u8) -> Self { Self(v) } }
         impl B { fn f(v: u8) -> Self { Self(v) } }",
    );
    equal(
        "struct A { v: u8 } struct B { v: u8 }
         impl A { fn f(v: u8) -> Self { Self { v } } }
         impl B { fn f(v: u8) -> Self { Self { v } } }",
    );
    equal(
        "struct A; struct B;
         impl A { fn f() -> Self { Self } }
         impl B { fn f() -> Self { Self } }",
    );
}

#[test]
fn owner_parameters_group_by_first_use_and_keep_their_roles() {
    let cases = [
        (
            "struct A<T>(T); struct B<T>(T);
             impl<T> A<T> { fn f() -> usize { core::mem::size_of::<T>() } }
             impl<T> B<T> { fn f() -> usize { core::mem::size_of::<T>() } }",
            COPIES,
        ),
        (
            "struct A<T>(T); struct B<T>(T);
             impl<T: Default> A<T> { fn f() -> T { T::default() } }
             impl<T: Default> B<T> { fn f() -> T { T::default() } }",
            COPIES,
        ),
        (
            "struct A<const N: usize>; struct B<const N: usize>;
             impl<const N: usize> A<N> { fn f() -> [u8; N] { [0; N] } }
             impl<const N: usize> B<N> { fn f() -> [u8; N] { [0; N] } }",
            COPIES,
        ),
        (
            "struct A<'a>(&'a u8); struct B<'a>(&'a u8);
             impl<'a> A<'a> { fn f(x: &'a u8) -> &'a u8 { x } }
             impl<'a> B<'a> { fn f(x: &'a u8) -> &'a u8 { x } }",
            COPIES,
        ),
        // The role is the argument position in the owner type, whatever the header lists.
        (
            "struct P<T, U>(T, U); struct Q<T, U>(T, U);
             impl<T, U> P<T, U> { fn f() -> usize { core::mem::size_of::<T>() } }
             impl<U, T> Q<T, U> { fn f() -> usize { core::mem::size_of::<T>() } }",
            COPIES,
        ),
        (
            "struct From<S>(S); struct NoFrom; struct A<'a, S, F, D>(&'a (S, F, D));
             impl<'a, S, Q, D> A<'a, S, From<Q>, D> { fn f() -> usize { core::mem::size_of::<D>() } }
             impl<'a, S, D> A<'a, S, NoFrom, D> { fn f() -> usize { core::mem::size_of::<D>() } }",
            COPIES,
        ),
        (
            "trait Fragment<B> {} struct A; struct C;
             impl<B> Fragment<B> for A { fn f() -> usize { core::mem::size_of::<B>() } }
             impl<B> Fragment<B> for C { fn f() -> usize { core::mem::size_of::<B>() } }",
            COPIES,
        ),
        (
            "struct A<T>(T); struct B<T>(T);
             impl<T: Into<U>, U> A<T> { fn f(x: T) -> U { x.into() } }
             impl<V, T: Into<V>> B<T> { fn f(x: T) -> V { x.into() } }",
            COPIES,
        ),
        // Grouping ignores the owner's shape, the role key keeps it.
        (
            "trait Table { fn table() -> u8; }
             impl<T: Table> Table for &T { fn f() -> u8 { T::table() } }
             impl<T: Table> Table for Box<T> { fn f() -> u8 { T::table() } }",
            ROLE_SPLIT,
        ),
        (
            "struct A<'a, T>(&'a T); struct B<T>(T);
             impl<'a, T> A<'a, T> { fn f(x: T) -> T { x } }
             impl<T> B<T> { fn f(x: T) -> T { x } }",
            ROLE_SPLIT,
        ),
        (
            "struct A<T, U>(T, U); struct B<T, U>(T, U);
             impl<T, U> A<T, U> { fn f() -> (usize, usize) { (core::mem::size_of::<T>(), core::mem::size_of::<U>()) } }
             impl<T, U> B<T, U> { fn f() -> (usize, usize) { (core::mem::size_of::<U>(), core::mem::size_of::<T>()) } }",
            ROLE_SPLIT,
        ),
        // Different parameters of one owner differ in role, in the owner type and outside it.
        (
            "struct A<T, U>(T, U);
             impl<T, U> A<T, U> { fn f() -> usize { core::mem::size_of::<T>() } }
             impl<T, U> A<T, U> { fn f() -> usize { core::mem::size_of::<U>() } }",
            ROLE_SPLIT,
        ),
        (
            "struct A<T>(T);
             impl<T, U, V> A<T> { fn f() -> usize { core::mem::size_of::<U>() } }
             impl<T, U, V> A<T> { fn f() -> usize { core::mem::size_of::<V>() } }",
            ROLE_SPLIT,
        ),
        // Two parameters stay apart from one parameter used twice.
        (
            "struct A<T, U>(T, U); struct B<T, U>(T, U);
             impl<T, U> A<T, U> { fn f() -> usize { core::mem::size_of::<T>() + core::mem::size_of::<U>() } }
             impl<T, U> B<T, U> { fn f() -> usize { core::mem::size_of::<T>() + core::mem::size_of::<T>() } }",
            DISTINCT,
        ),
        // An owner parameter stays apart from a module type of the same spelling.
        (
            "struct T; struct A<T>(T); struct B;
             impl<T> A<T> { fn f() -> usize { core::mem::size_of::<T>() } }
             impl B { fn f() -> usize { core::mem::size_of::<T>() } }",
            DISTINCT,
        ),
    ];
    for (src, expected) in cases {
        assert_eq!(same(src), expected, "{src}");
    }
}

#[test]
fn owner_roles_follow_the_owner_type_structure() {
    let cases = [
        // References, slices and pointers keep their element role.
        (
            "trait Tr { fn f(); }
             impl<'a, T> Tr for &'a T { fn f(x: &'a T) -> &'a T { x } }
             impl<'a, T> Tr for &'a mut T { fn f(x: &'a T) -> &'a T { x } }",
            COPIES,
        ),
        (
            "trait Tr { fn f(); }
             impl<T> Tr for [T] { fn f() -> usize { core::mem::size_of::<T>() } }
             impl<T> Tr for *const T { fn f() -> usize { core::mem::size_of::<T>() } }",
            COPIES,
        ),
        // Array element and length, and tuple positions, are separate roles.
        (
            "trait Tr { fn f(); }
             impl<T, const N: usize> Tr for [T; N] { fn f() -> usize { N } }
             impl<T, const N: usize> Tr for Box<[T; N]> { fn f() -> usize { N } }",
            ROLE_SPLIT,
        ),
        (
            "trait Tr { fn f(); }
             impl<T, U> Tr for (T, U) { fn f() -> usize { core::mem::size_of::<U>() } }
             impl<T, U> Tr for (U, T) { fn f() -> usize { core::mem::size_of::<U>() } }",
            ROLE_SPLIT,
        ),
        // Lifetime arguments take their argument position.
        (
            "trait Tr { fn f(); }
             struct Pair<'a, 'b>(&'a u8, &'b u8);
             impl<'a, 'b> Tr for Pair<'a, 'b> { fn f(x: &'a u8) -> &'a u8 { x } }
             impl<'a, 'b> Tr for Pair<'b, 'a> { fn f(x: &'a u8) -> &'a u8 { x } }",
            ROLE_SPLIT,
        ),
        // Trait objects number their parameter occurrences in order.
        (
            "trait Tr { fn f(); }
             impl<A, R> Tr for Box<dyn Fn(A) -> R> { fn f() -> usize { core::mem::size_of::<R>() } }
             impl<A, R> Tr for std::rc::Rc<dyn Fn(A) -> R> { fn f() -> usize { core::mem::size_of::<R>() } }",
            COPIES,
        ),
        (
            "trait Tr { fn f(); }
             impl<A, R> Tr for Box<dyn Fn(A) -> R> { fn f() -> usize { core::mem::size_of::<R>() } }
             impl<A, R> Tr for Box<dyn Fn(R) -> A> { fn f() -> usize { core::mem::size_of::<R>() } }",
            ROLE_SPLIT,
        ),
        (
            "trait Tr { fn f(); }
             impl<'a> Tr for Box<dyn core::any::Any + 'a> { fn f(x: &'a u8) -> &'a u8 { x } }
             impl<'a> Tr for std::rc::Rc<dyn core::any::Any + 'a> { fn f(x: &'a u8) -> &'a u8 { x } }",
            COPIES,
        ),
        // An associated type argument numbers its parameter occurrences in order.
        (
            "trait Tr { fn f(); }
             impl<T> Tr for Box<Iterator<Item = T>> { fn f() -> usize { core::mem::size_of::<T>() } }
             impl<T> Tr for std::rc::Rc<Iterator<Item = T>> { fn f() -> usize { core::mem::size_of::<T>() } }",
            COPIES,
        ),
    ];
    for (src, expected) in cases {
        assert_eq!(same(src), expected, "{src}");
    }
}

#[test]
fn trait_default_self_matches_across_traits() {
    equal(
        "trait P: Sized { fn f(self) -> Self { self } }
         trait Q: Sized { fn f(self) -> Self { self } }",
    );
}

#[test]
fn value_position_self_members_keep_their_owner() {
    unequal(
        "struct A; struct B;
         impl A { fn h() -> u32 { 1 } fn f() -> u32 { Self::h() } }
         impl B { fn h() -> u32 { 1 } fn f() -> u32 { Self::h() } }",
    );
    unequal(
        "struct A; struct B;
         impl A { const K: u32 = 1; fn f() -> u32 { Self::K } }
         impl B { const K: u32 = 1; fn f() -> u32 { Self::K } }",
    );
    unequal(
        "enum A { X } enum B { X }
         impl A { fn f() -> Self { Self::X } }
         impl B { fn f() -> Self { Self::X } }",
    );
    unequal(
        "struct A; struct B;
         impl A { fn h() -> u32 { 1 } fn f() -> u32 { m!(Self::h()) } }
         impl B { fn h() -> u32 { 2 } fn f() -> u32 { m!(Self::h()) } }",
    );
    // The same member call under one owner still matches.
    equal(
        "struct A;
         impl A { fn h() -> u32 { 1 } fn f() -> u32 { Self::h() } }
         impl A { fn f() -> u32 { Self::h() } }",
    );
}

#[test]
fn explicit_owner_names_print_like_self() {
    equal(
        "struct A;
         impl A { fn f() -> Self { Self } }
         impl A { fn f() -> A { A } }",
    );
    equal(
        "struct A; struct B;
         impl A { fn f() -> A { A } }
         impl B { fn f() -> B { B } }",
    );
    equal(
        "struct A;
         impl A { fn h() -> u32 { 1 } fn f() -> u32 { Self::h() } }
         impl A { fn f() -> u32 { A::h() } }",
    );
    unequal(
        "struct A; struct B;
         impl A { fn h() -> u32 { 1 } fn f() -> u32 { A::h() } }
         impl B { fn h() -> u32 { 1 } fn f() -> u32 { B::h() } }",
    );
}

#[test]
fn other_types_keep_their_identity_inside_an_owner() {
    equal(
        "struct A; struct B; struct C;
         impl A { fn f() -> C { C } }
         impl B { fn f() -> C { C } }",
    );
    unequal(
        "struct A; struct B;
         impl A { fn f() -> A { A } }
         impl B { fn f() -> A { A } }",
    );
    unequal(
        "mod m { pub struct S; }
         struct S;
         impl S { fn f() -> S { S } }
         impl S { fn f() -> m::S { m::S } }",
    );
}

#[test]
fn owner_parameter_spellings_in_macro_payloads_stay_literal() {
    let plain = |a: &str, b: &str| {
        format!(
            "struct A; struct B;
             impl A {{ fn f() -> &'static str {{ stringify!({a}) }} }}
             impl B {{ fn f() -> &'static str {{ stringify!({b}) }} }}"
        )
    };
    let prefix = "__dejadoc_owner_param_";
    let escaped = "__dejadoc_escaped___dejadoc_owner_param_";
    assert_eq!(
        same(&plain(&format!("{prefix}a"), &format!("{prefix}b"))),
        DISTINCT
    );
    assert_eq!(
        same(&plain(&format!("{prefix}a"), &format!("{escaped}a"))),
        DISTINCT
    );
    assert_eq!(
        same(&plain(&format!("{prefix}a"), &format!("{prefix}a"))),
        COPIES
    );
    let generic = |a: &str, b: &str| {
        format!(
            "struct A<T>(T); struct B<U>(U);
             impl<T> A<T> {{ fn f(x: T) -> T {{ let _ = stringify!({a}); x }} }}
             impl<U> B<U> {{ fn f(x: U) -> U {{ let _ = stringify!({b}); x }} }}"
        )
    };
    assert_eq!(
        same(&generic(&format!("{prefix}0"), &format!("{prefix}1"))),
        DISTINCT
    );
    assert_eq!(
        same(&generic(&format!("{prefix}0"), &format!("{prefix}0"))),
        COPIES
    );
}

#[test]
fn owner_parameter_spellings_stay_literal_in_every_syntax_position() {
    let prefix = "__dejadoc_owner_param_";
    for (parameters, body) in [
        ("s: S", "let _ = s.{X}; 1u32"),
        ("", "#[observer({X})] let _ = 1u32; 1u32"),
        ("", "let _ = builtin # offset_of(S, {X}); 1u32"),
        ("_: &dyn* {X}", "1u32"),
        ("y: u32", "let box {X} = y; 1u32"),
        ("", "macro {X}() {{}} 1u32"),
        ("", "impl S {{ fn {X}(); }} 1u32"),
        ("", "trait T {{ fn g(); default fn {X}() {{}} }} 1u32"),
    ] {
        let src = |a: &str, b: &str| {
            let side = |name: &str| (parameters.replace("{X}", name), body.replace("{X}", name));
            let ((left_parameters, left), (right_parameters, right)) = (side(a), side(b));
            format!(
                "struct S; struct A; struct B;
                 impl A {{ fn f({left_parameters}) -> u32 {{ {left} }} }}
                 impl B {{ fn f({right_parameters}) -> u32 {{ {right} }} }}"
            )
            .replace("{{", "{")
            .replace("}}", "}")
        };
        let a = format!("{prefix}a");
        let b = format!("{prefix}b");
        assert_eq!(same(&src(&a, &b)), DISTINCT, "{body}");
        assert_eq!(same(&src(&a, &a)), COPIES, "{body}");
    }
}
