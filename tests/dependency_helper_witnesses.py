"""Bounded compiler witnesses for the M6 local helper scheduling."""

import argparse
import hashlib
import json
import pathlib
import sys
import time

from dependency_witnesses import (
    DEFAULT_TIMEOUT,
    EDITIONS,
    OVERFLOW_CHECKS,
    OPTIMIZATIONS,
    SIDES,
    run_matrix,
)

U32_MAX = 0xFFFFFFFF
ORIGINAL_ID = "D07_local_pure_calls"
ORIGINAL_INPUTS = (0, 1, 15, 16, 31, 255, 65535, U32_MAX)
ORIGINAL_MAIN = (
    "fn main(){for input in [0,1,15,16,31,255,65535,u32::MAX]"
    '{println!("{:?}",f(input));}}'
)
INPUTS = (0, 1, 15, 16, 31, 255, 65535, U32_MAX)
U64_MAX = 0xFFFFFFFFFFFFFFFF
U64_INPUTS = (0, 1, 15, 16, 31, 255, 65535, U64_MAX)
RECURSION_INPUTS = (3, 7, 15, 16, 31, 255)


def _debug(value):
    if isinstance(value, tuple):
        return "({0}, {1})".format(*value)
    return str(value)


def _lines(values):
    return "".join(_debug(value) + "\n" for value in values)


def _main(inputs, max_token, call):
    parts = [
        max_token if value in (U32_MAX, U64_MAX) else str(value) for value in inputs
    ]
    return (
        "fn main(){for input in ["
        + ",".join(parts)
        + "]{println!(\"{:?}\","
        + call
        + "(input));}}"
    )


def _program(fn, inputs, max_token="u32::MAX", call="f"):
    return fn + "\n" + _main(inputs, max_token, call)


def _origin_value(input):
    return (input & 15, input >> 4)


def _origin_swapped(input):
    return (input >> 4, input & 15)


def _effect_lines(input, reversed_order):
    if reversed_order:
        return "{0}\n{1}\n".format(input ^ 1, input)
    return "{0}\n{1}\n".format(input, input ^ 1)


def _effect_value(input):
    return (input & 15, (input ^ 1) & 15)

def _effect_source(reversed_order):
    body = (
        "let a=pure(effect(input));let b=pure(effect(input^1u32));(a,b)"
        if not reversed_order
        else "let b=pure(effect(input^1u32));let a=pure(effect(input));(a,b)"
    )
    return (
        "fn pure(x:u32)->u32{x&15u32} fn effect(x:u32)->u32{println!(\"{x}\");x}"
        "fn f(input:u32)->(u32,u32){"
        + body
        + "}"
    )


def _effect_stdout(reversed_order):
    return "".join(
        _effect_lines(input, reversed_order) + _debug(_effect_value(input)) + "\n"
        for input in INPUTS
    )


def _transitive_value(input):
    return (((input & 15) ^ 3) | 4, (input ^ 1) & 15)


def _structured_value(input):
    def low(value):
        return value if value < 16 else value & 7

    return (low(input), low(input ^ 1))


def _low_value(input):
    return (input & 15, (input ^ 1) & 15)


def _shift_value(input):
    return (input & 7, input >> 5)


def _repeated_local_module_source(swapped):
    origin = "pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4u32}"
    shift = "pub fn low(x:u32)->u32{x&7u32}pub fn high(x:u32)->u32{x>>5u32}"
    first, second = (shift, origin) if swapped else (origin, shift)
    return (
        "fn first(input:u32)->(u32,u32){mod m{"
        + first
        + "}let a=m::low(input);let b=m::high(input);(a,b)}"
        "fn second(input:u32)->(u32,u32){mod m{"
        + second
        + "}let a=m::low(input);let b=m::high(input);(a,b)}"
        "fn main(){for input in [0,1,15,16,31,255,65535,u32::MAX]"
        "{println!(\"{:?}\",first(input));println!(\"{:?}\",second(input));}}"
    )


def _repeated_local_module_stdout(swapped):
    first, second = (
        (_shift_value, _origin_value) if swapped else (_origin_value, _shift_value)
    )
    return "".join(
        _debug(first(input)) + "\n" + _debug(second(input)) + "\n"
        for input in INPUTS
    )


def _nearer_glob_source(reversed_order):
    body = (
        "let a=low(x);let b=low(x^1u32);(a,b)"
        if not reversed_order
        else "let b=low(x^1u32);let a=low(x);(a,b)"
    )
    return (
        "mod far{pub fn low(x:u32)->u32{x>>4u32}}"
        "mod near{pub fn low(x:u32)->u32{println!(\"{x}\");x&15u32}}"
        "use crate::far::*;"
        "fn first(x:u32)->(u32,u32){use crate::near::*;"
        + body
        + "}"
    )


def _constructor_mask_source(construction_first):
    statements = (
        "let a=low(x);let b=low(x^1u32);"
        if construction_first
        else "let b=low(x^1u32);let a=low(x);"
    )
    return (
        "fn low(x:u32)->u32{x&15u32}"
        "fn f(x:u32)->(u32,u32){struct low(u32);"
        'impl Drop for low{fn drop(&mut self){println!("D:{}",self.0);}}'
        + statements
        + "(a.0,b.0)}"
    )


def _deep_local_module_source(panicking, reversed_order):
    root = (
        "mod m{pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}}"
        if panicking
        else ""
    )
    inner = (
        'pub fn low(_:u32)->u32{panic!("low")}pub fn high(_:u32)->u32{panic!("high")}'
        if panicking
        else "pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}"
    )
    body = (
        "let a=m::low(x);let b=m::high(x);(a,b)"
        if not reversed_order
        else "let b=m::high(x);let a=m::low(x);(a,b)"
    )
    return (
        root
        + "fn f(x:u32)->(u32,u32){"
        + "{" * 40
        + "mod m{"
        + inner
        + "}"
        + body
        + "}" * 40
        + "}"
    )


_panic_main = (
    'fn main(){std::panic::set_hook(Box::new(|info|{'
    'if let Some(s)=info.payload().downcast_ref::<&str>(){println!("{s}");'
    '}}));f(0u32);}'
)


H_CASES = [
    {
        "id": "H01_origin_results",
        "a": _program(
            "fn one(x:u32)->u32{x&15u32} fn two(x:u32)->u32{x>>4}"
            "fn f(input:u32)->(u32,u32){(one(input),two(input))}",
            INPUTS,
        ),
        "b": _program(
            "fn one(x:u32)->u32{x>>4} fn two(x:u32)->u32{x&15u32}"
            "fn f(input:u32)->(u32,u32){(one(input),two(input))}",
            INPUTS,
        ),
        "expected": "different",
        "expect": {
            "a": {"exit": 0, "stdout": _lines(_origin_value(v) for v in INPUTS)},
            "b": {"exit": 0, "stdout": _lines(_origin_swapped(v) for v in INPUTS)},
        },
    },
    {
        "id": "H02_argument_effects",
        "a": _program(_effect_source(False), INPUTS),
        "b": _program(_effect_source(True), INPUTS),
        "expected": "different",
        "expect": {
            "a": {"exit": 0, "stdout": _effect_stdout(False)},
            "b": {"exit": 0, "stdout": _effect_stdout(True)},
        },
    },
    {
        "id": "H03_transitive_chain",
        "a": _program(
            "fn low(x:u32)->u32{x&15u32} fn mid(x:u32)->u32{low(x)^3u32}"
            " fn high(x:u32)->u32{mid(x)|4u32}"
            "fn f(input:u32)->(u32,u32){let a=high(input);let b=low(input^1u32);(a,b)}",
            INPUTS,
        ),
        "b": _program(
            "fn low(x:u32)->u32{x&15u32} fn mid(x:u32)->u32{low(x)^3u32}"
            " fn high(x:u32)->u32{mid(x)|4u32}"
            "fn f(input:u32)->(u32,u32){let b=low(input^1u32);let a=high(input);(a,b)}",
            INPUTS,
        ),
        "expected": "same",
        "expect": {
            "a": {"exit": 0, "stdout": _lines(_transitive_value(v) for v in INPUTS)},
            "b": {"exit": 0, "stdout": _lines(_transitive_value(v) for v in INPUTS)},
        },
    },
    {
        "id": "H04_structured_if",
        "a": _program(
            "fn low(x:u32)->u32{if x<16u32{x&15u32}else{let v=x&7u32;v}}"
            "fn f(input:u32)->(u32,u32){let a=low(input);let b=low(input^1u32);(a,b)}",
            INPUTS,
        ),
        "b": _program(
            "fn low(x:u32)->u32{if x<16u32{x&15u32}else{let v=x&7u32;v}}"
            "fn f(input:u32)->(u32,u32){let b=low(input^1u32);let a=low(input);(a,b)}",
            INPUTS,
        ),
        "expected": "same",
        "expect": {
            "a": {"exit": 0, "stdout": _lines(_structured_value(v) for v in INPUTS)},
            "b": {"exit": 0, "stdout": _lines(_structured_value(v) for v in INPUTS)},
        },
    },
    {
        "id": "H05_module_import",
        "a": _program(
            "mod inner{pub fn low(x:u32)->u32{x&15u32}}"
            " mod outer{use super::inner::low;"
            "pub fn first(input:u32)->(u32,u32){let a=low(input);let b=low(input^1u32);(a,b)}}",
            INPUTS,
            call="outer::first",
        ),
        "b": _program(
            "mod inner{pub fn low(x:u32)->u32{x&15u32}}"
            " mod outer{use super::inner::low;"
            "pub fn first(input:u32)->(u32,u32){let b=low(input^1u32);let a=low(input);(a,b)}}",
            INPUTS,
            call="outer::first",
        ),
        "expected": "same",
        "expect": {
            "a": {"exit": 0, "stdout": _lines(_low_value(v) for v in INPUTS)},
            "b": {"exit": 0, "stdout": _lines(_low_value(v) for v in INPUTS)},
        },
    },
    {
        "id": "H06_primitive_alias_raw",
        "a": _program(
            "mod m{use core::primitive::u64 as W;"
            " fn r#type(x:W)->W{x&15u64}"
            " pub fn first(input:W)->(W,W){let a=r#type(input);let b=r#type(input^1u64);(a,b)}}",
            U64_INPUTS,
            max_token="u64::MAX",
            call="m::first",
        ),
        "b": _program(
            "mod m{use core::primitive::u64 as W;"
            " fn r#type(x:W)->W{x&15u64}"
            " pub fn first(input:W)->(W,W){let b=r#type(input^1u64);let a=r#type(input);(a,b)}}",
            U64_INPUTS,
            max_token="u64::MAX",
            call="m::first",
        ),
        "expected": "same",
        "expect": {
            "a": {"exit": 0, "stdout": _lines(_low_value(v) for v in U64_INPUTS)},
            "b": {"exit": 0, "stdout": _lines(_low_value(v) for v in U64_INPUTS)},
        },
    },
    {
        "id": "H07_closure_shadow",
        "a": _program(
            "fn helper(x:u32)->u32{x&15u32}"
            "fn f(x:u32)->(u32,u32){let helper=|v:u32|{println!(\"{v}\");v};"
            "let a=helper(x);let b=helper(x^1u32);(a,b)}",
            INPUTS,
        ),
        "b": _program(
            "fn helper(x:u32)->u32{x&15u32}"
            "fn f(x:u32)->(u32,u32){let helper=|v:u32|{println!(\"{v}\");v};"
            "let b=helper(x^1u32);let a=helper(x);(a,b)}",
            INPUTS,
        ),
        "expected": "different",
        "expect": {
            "a": {
                "exit": 0,
                "stdout": "".join(
                    _effect_lines(v, False) + _debug((v, v ^ 1)) + "\n"
                    for v in INPUTS
                ),
            },
            "b": {
                "exit": 0,
                "stdout": "".join(
                    _effect_lines(v, True) + _debug((v, v ^ 1)) + "\n"
                    for v in INPUTS
                ),
            },
        },
    },
    {
        "id": "H08_track_caller_arguments",
        "a": _program(
            "#[track_caller] fn observed(x:u32)->u32{x&15u32}"
            " fn effect(x:u32)->u32{println!(\"{x}\");x}"
            "fn f(x:u32)->(u32,u32){let a=observed(effect(x));let b=observed(effect(x^1u32));(a,b)}",
            INPUTS,
        ),
        "b": _program(
            "#[track_caller] fn observed(x:u32)->u32{x&15u32}"
            " fn effect(x:u32)->u32{println!(\"{x}\");x}"
            "fn f(x:u32)->(u32,u32){let b=observed(effect(x^1u32));let a=observed(effect(x));(a,b)}",
            INPUTS,
        ),
        "expected": "different",
        "expect": {
            "a": {
                "exit": 0,
                "stdout": "".join(
                    _effect_lines(v, False) + _debug(_effect_value(v)) + "\n"
                    for v in INPUTS
                ),
            },
            "b": {
                "exit": 0,
                "stdout": "".join(
                    _effect_lines(v, True) + _debug(_effect_value(v)) + "\n"
                    for v in INPUTS
                ),
            },
        },
    },
    {
        "id": "H09_recursion_unexecuted",
        "a": _program(
            "fn rec(x:u32)->u32{if x==0u32{rec(x)}else{x}}"
            " fn effect(x:u32)->u32{println!(\"{x}\");x}"
            "fn f(x:u32)->(u32,u32){let a=rec(effect(x));let b=rec(effect(x^1u32));(a,b)}",
            RECURSION_INPUTS,
        ),
        "b": _program(
            "fn rec(x:u32)->u32{if x==0u32{rec(x)}else{x}}"
            " fn effect(x:u32)->u32{println!(\"{x}\");x}"
            "fn f(x:u32)->(u32,u32){let b=rec(effect(x^1u32));let a=rec(effect(x));(a,b)}",
            RECURSION_INPUTS,
        ),
        "expected": "different",
        "expect": {
            "a": {
                "exit": 0,
                "stdout": "".join(
                    _effect_lines(v, False) + _debug((v, v ^ 1)) + "\n"
                    for v in RECURSION_INPUTS
                ),
            },
            "b": {
                "exit": 0,
                "stdout": "".join(
                    _effect_lines(v, True) + _debug((v, v ^ 1)) + "\n"
                    for v in RECURSION_INPUTS
                ),
            },
        },
    },
    {
        "id": "H13_repeated_local_modules",
        "a": _repeated_local_module_source(False),
        "b": _repeated_local_module_source(True),
        "expected": "different",
        "expect": {
            "a": {"exit": 0, "stdout": _repeated_local_module_stdout(False)},
            "b": {"exit": 0, "stdout": _repeated_local_module_stdout(True)},
        },
    },
    {
        "id": "H14_local_module_shadow_root",
        "a": _program(
            "mod m{pub fn low(x:u32)->u32{x>>4u32}pub fn high(x:u32)->u32{x&15u32}}"
            "fn first(input:u32)->(u32,u32){mod m{pub fn low(x:u32)->u32{x&15u32}"
            "pub fn high(x:u32)->u32{x>>4u32}}"
            "let a=m::low(input);let b=m::high(input);(a,b)}",
            INPUTS,
            call="first",
        ),
        "b": _program(
            "mod m{pub fn low(x:u32)->u32{x>>4u32}pub fn high(x:u32)->u32{x&15u32}}"
            "fn first(input:u32)->(u32,u32){mod m{pub fn low(x:u32)->u32{x&15u32}"
            "pub fn high(x:u32)->u32{x>>4u32}}"
            "let b=m::high(input);let a=m::low(input);(a,b)}",
            INPUTS,
            call="first",
        ),
        "expected": "same",
        "expect": {
            side: {"exit": 0, "stdout": _lines(_origin_value(v) for v in INPUTS)}
            for side in SIDES
        },
    },
    {
        "id": "H15_local_module_super",
        "a": _program(
            "mod a{pub fn base(x:u32)->u32{x&15u32}"
            "pub fn first(input:u32)->(u32,u32){mod m{pub fn low(x:u32)->u32{super::base(x)}"
            "pub fn high(x:u32)->u32{x>>4u32}}let p=m::low(input);let q=m::high(input);(p,q)}}",
            INPUTS,
            call="a::first",
        ),
        "b": _program(
            "mod b{pub fn base(x:u32)->u32{x>>4u32}"
            "pub fn first(input:u32)->(u32,u32){mod m{pub fn low(x:u32)->u32{super::base(x)}"
            "pub fn high(x:u32)->u32{x&15u32}}let p=m::low(input);let q=m::high(input);(p,q)}}",
            INPUTS,
            call="b::first",
        ),
        "expected": "different",
        "expect": {
            "a": {"exit": 0, "stdout": _lines(_origin_value(v) for v in INPUTS)},
            "b": {"exit": 0, "stdout": _lines(_origin_swapped(v) for v in INPUTS)},
        },
    },
    {
        "id": "H16_local_module_alias_imports",
        "a": _program(
            "mod m{pub fn low(x:u32)->u32{x>>4u32}pub fn high(x:u32)->u32{x&15u32}}"
            "fn first(input:u32)->(u32,u32){mod m{pub fn low(x:u32)->u32{x&15u32}"
            "pub fn high(x:u32)->u32{x>>4u32}}use m::{low as h,high as k};"
            "use m as mm;let a=h(input);let b=mm::high(input);(a,b)}",
            INPUTS,
            call="first",
        ),
        "b": _program(
            "mod m{pub fn low(x:u32)->u32{x>>4u32}pub fn high(x:u32)->u32{x&15u32}}"
            "fn first(input:u32)->(u32,u32){mod m{pub fn low(x:u32)->u32{x&15u32}"
            "pub fn high(x:u32)->u32{x>>4u32}}use m::{low as h,high as k};"
            "use m as mm;let a=k(input);let b=mm::low(input);(a,b)}",
            INPUTS,
            call="first",
        ),
        "expected": "different",
        "expect": {
            "a": {"exit": 0, "stdout": _lines(_origin_value(v) for v in INPUTS)},
            "b": {"exit": 0, "stdout": _lines(_origin_swapped(v) for v in INPUTS)},
        },
    },
    {
        "id": "H17_qualified_primitive_alias",
        "a": _program(
            "type W=core::primitive::u32;"
            "mod m{type W=core::primitive::u64;"
            "fn low(x:W)->W{x&15u64}"
            "pub fn first(input:W)->(W,W){let a=low(input);let b=low(input^1u64);(a,b)}}",
            U64_INPUTS,
            max_token="u64::MAX",
            call="m::first",
        ),
        "b": _program(
            "type W=core::primitive::u32;"
            "mod m{type W=core::primitive::u64;"
            "fn low(x:W)->W{x&15u64}"
            "pub fn first(input:W)->(W,W){let b=low(input^1u64);let a=low(input);(a,b)}}",
            U64_INPUTS,
            max_token="u64::MAX",
            call="m::first",
        ),
        "expected": "same",
        "expect": {
            side: {"exit": 0, "stdout": _lines(_low_value(v) for v in U64_INPUTS)}
            for side in SIDES
        },
    },
    {
        "id": "H18_nearer_glob_impure",
        "a": _program(_nearer_glob_source(False), INPUTS, call="first"),
        "b": _program(_nearer_glob_source(True), INPUTS, call="first"),
        "expected": "different",
        "expect": {
            "a": {"exit": 0, "stdout": _effect_stdout(False)},
            "b": {"exit": 0, "stdout": _effect_stdout(True)},
        },
    },
    {
        "id": "H19_constructor_mask_drop_order",
        "a": _program(_constructor_mask_source(True), INPUTS),
        "b": _program(_constructor_mask_source(False), INPUTS),
        "expected": "different",
        "expect": {
            "a": {
                "exit": 0,
                "stdout": "".join(f"D:{v ^ 1}\nD:{v}\n({v}, {v ^ 1})\n" for v in INPUTS),
            },
            "b": {
                "exit": 0,
                "stdout": "".join(f"D:{v}\nD:{v ^ 1}\n({v}, {v ^ 1})\n" for v in INPUTS),
            },
        },
    },
    {
        "id": "H20_deep_nearer_module_panic",
        "a": _deep_local_module_source(True, False) + _panic_main,
        "b": _deep_local_module_source(True, True) + _panic_main,
        "expected": "different",
        "expect": {
            "a": {"exit": 101, "stdout": "low\n"},
            "b": {"exit": 101, "stdout": "high\n"},
        },
    },
    {
        "id": "H21_deep_total_local_module",
        "a": _program(_deep_local_module_source(False, False), INPUTS),
        "b": _program(_deep_local_module_source(False, True), INPUTS),
        "expected": "same",
        "expect": {
            side: {"exit": 0, "stdout": _lines(_origin_value(v) for v in INPUTS)}
            for side in SIDES
        },
    },
]


def _summary_resource(extra, reverse):
    body = "true;" * 8185
    if extra == 1:
        body = "!p;" + body[5:]
    elif extra == 2:
        body += "p;"
    statements = (
        "let b=k(!p);let a=h(p);"
        if reverse else "let a=h(p);let b=k(!p);"
    )
    return (
        f"fn f(p:bool)->(bool,bool){{{statements}(a,b)}}"
        f"fn h(p:bool)->bool{{{body}g(p);p}}"
        "fn g(p:bool)->bool{!p}fn k(p:bool)->bool{p}"
        'fn main(){for p in [true,false]{println!("{:?}",f(p));}}'
    )


for _extra in range(3):
    H_CASES.append({
        "id": f"H10_summary_nodes_{16384 + _extra}",
        "a": _summary_resource(_extra, False),
        "b": _summary_resource(_extra, True),
        "expected": "same",
        "expect": {
            side: {"exit": 0, "stdout": "(true, false)\n(false, true)\n"}
            for side in SIDES
        },
    })


H_CASES.append({
    "id": "H11_caller_location",
    "a": '#[track_caller] fn observed()->u32{std::panic::Location::caller().line()}\n'
         "fn f()->(u32,u32){\n"
         "let a=observed();\n"
         "let b=observed();\n"
         "(a,b)\n}\n"
         'fn main(){println!("{:?}",f());}',
    "b": '#[track_caller] fn observed()->u32{std::panic::Location::caller().line()}\n'
         "fn f()->(u32,u32){\n"
         "let b=observed();\n"
         "let a=observed();\n"
         "(a,b)\n}\n"
         'fn main(){println!("{:?}",f());}',
    "expected": "different",
    "expect": {
        "a": {"exit": 0, "stdout": "(3, 4)\n"},
        "b": {"exit": 0, "stdout": "(4, 3)\n"},
    },
})


def _diamond_resource(extra, reverse):
    tail = "!p" if extra else "p"
    base = "!p;" + "true;" * 8177 + tail
    statements = (
        "let b=k(!p);let a=h(p);" if reverse else "let a=h(p);let b=k(!p);"
    )
    return (
        f"fn f(p:bool)->(bool,bool){{{statements}(a,b)}}"
        "fn h(p:bool)->bool{left(p);right(p);p}"
        "fn left(p:bool)->bool{base(p)}fn right(p:bool)->bool{base(p)}"
        f"fn base(p:bool)->bool{{{base}}}"
        "fn k(p:bool)->bool{p}"
        'fn main(){for p in [true,false]{println!("{:?}",f(p));}}'
    )


for _extra in range(2):
    H_CASES.append({
        "id": f"H12_diamond_nodes_{16384 + _extra}",
        "a": _diamond_resource(_extra, False),
        "b": _diamond_resource(_extra, True),
        "expected": "same",
        "expect": {
            side: {"exit": 0, "stdout": "(true, false)\n(false, true)\n"}
            for side in SIDES
        },
    })


def _xor_tree(leaf, count):
    if count == 1:
        return leaf
    return "({0}^{1})".format(_xor_tree(leaf, count // 2), _xor_tree(leaf, count - count // 2))


def _original_if_source(count, reversed_order):
    tree = _xor_tree("p", count)
    statements = (
        "let b=k(!p);let a=h(p);" if reversed_order else "let a=h(p);let b=k(!p);"
    )
    return (
        f"fn h(p:bool)->bool{{if true{{p}}else{{{tree}}}}}"
        "fn k(p:bool)->bool{p}"
        f"fn f(p:bool)->(bool,bool){{{statements}(a,b)}}"
        'fn main(){for p in [true,false]{println!("{:?}",f(p));}}'
    )


for _index, (_count, _nodes) in enumerate(((5458, 16383), (5459, 16386))):
    case = {
        "id": f"H{22 + _index}_original_if_nodes_{_nodes}",
        "a": _original_if_source(_count, False),
        "b": _original_if_source(_count, True),
        "expected": "same",
        "expect": {
            side: {"exit": 0, "stdout": "(true, false)\n(false, true)\n"}
            for side in SIDES
        },
    }
    if _count == 5459:
        case["note"] = (
            "over the 16,384-node original proof cap the pair keeps source "
            "order, so the equal native result is a resource-boundary "
            "witness, not a canonical scheduling admission"
        )
    H_CASES.append(case)


def _load_original(path):
    try:
        cases = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read {path} {error}")
    if not isinstance(cases, list):
        raise ValueError("cases must be a JSON list")
    found = {case.get("id"): case for case in cases if isinstance(case, dict)}
    case = found.get(ORIGINAL_ID)
    if case is None:
        raise ValueError(f"missing original case {ORIGINAL_ID}")
    if case.get("expected") != "same":
        raise ValueError(f"original case {ORIGINAL_ID} drifted from same")
    for side in SIDES:
        source = case.get(side)
        if not isinstance(source, str) or ORIGINAL_MAIN not in source:
            raise ValueError(
                f"original case {ORIGINAL_ID} side {side} drifted from "
                "the studied main loop"
            )
    stdout = _lines(_origin_value(input) for input in ORIGINAL_INPUTS)
    return [
        {
            "id": ORIGINAL_ID,
            "a": case["a"],
            "b": case["b"],
            "expected": "same",
            "expect": {
                "a": {"exit": 0, "stdout": stdout},
                "b": {"exit": 0, "stdout": stdout},
            },
        }
    ]


def _verify_independent(report, cases):
    lookup = {row["request_id"]: row for row in report["results"]}
    checks = []
    for case in cases:
        case_id = case["id"]
        expect = case.get("expect", {})
        for edition in EDITIONS:
            for optimization in OPTIMIZATIONS:
                for overflow_checks in OVERFLOW_CHECKS:
                    for side in SIDES:
                        spec = expect.get(side)
                        if spec is None:
                            continue
                        row_id = (
                            f"{report['request_id']}/{case_id}/{edition}"
                            f"/{optimization}/{int(overflow_checks)}/{side}"
                        )
                        row = lookup.get(row_id)
                        run = row.get("run") if row else None
                        matches = (
                            row is not None
                            and row["compile"]["exit"] == 0
                            and not row["compile"]["timed_out"]
                            and run is not None
                            and not run["timed_out"]
                            and run["exit"] == spec["exit"]
                        )
                        expected_sha = None
                        if matches and "stdout" in spec:
                            matches = run["stdout"] == spec["stdout"]
                            expected_sha = hashlib.sha256(
                                spec["stdout"].encode("utf-8")
                            ).hexdigest()
                        checks.append(
                            {
                                "case_id": case_id,
                                "side": side,
                                "edition": edition,
                                "optimization": optimization,
                                "overflow_checks": overflow_checks,
                                "request_id": row_id,
                                "expected_exit": spec["exit"],
                                "exit": run["exit"] if run else None,
                                "expected_stdout_sha256": expected_sha,
                                "stdout_sha256": hashlib.sha256(
                                    (run["stdout"] if run else "")
                                    .encode("utf-8")
                                ).hexdigest(),
                                "matches": matches,
                            }
                        )
    return checks


def main(argv=None):
    """Run the helper witness matrix and return zero when every check matches."""
    parser = argparse.ArgumentParser(
        prog="dependency_helper_witnesses",
        description="Run the M6 helper scheduling witness matrix.",
    )
    parser.add_argument(
        "--original-cases",
        default=str(
            pathlib.Path(__file__).resolve().with_name("fixtures")
            .joinpath("dependency", "cases.json")
        ),
        help="fixture path holding the original D07 pair",
    )
    parser.add_argument("--request-id", default="M6-HELPER-WITNESS-01")
    parser.add_argument("--toolchain", default="stable")
    parser.add_argument(
        "--timeout",
        type=float,
        default=1200.0,
        help="global wall clock budget in seconds",
    )
    parser.add_argument(
        "--output",
        help="report path. omit to print the report on standard output",
    )
    parser.add_argument(
        "--case",
        action="append",
        default=None,
        help="case id filter. repeatable",
    )
    args = parser.parse_args(argv)
    if args.timeout <= 0:
        print("dependency_helper_witnesses timeout must be positive", file=sys.stderr)
        return 2
    try:
        cases = _load_original(args.original_cases)
        cases.extend(H_CASES)
    except ValueError as error:
        print(f"dependency_helper_witnesses invalid fixtures {error}", file=sys.stderr)
        return 2
    if args.case:
        known = {case["id"] for case in cases}
        unknown = [value for value in args.case if value not in known]
        if unknown:
            print(
                f"dependency_helper_witnesses unknown case ids "
                f"{', '.join(unknown)}",
                file=sys.stderr,
            )
            return 2
        wanted = set(args.case)
        cases = [case for case in cases if case["id"] in wanted]
        if not cases:
            print(
                "dependency_helper_witnesses case filter removed all cases",
                file=sys.stderr,
            )
            return 2
    started = time.monotonic()
    report = run_matrix(
        cases,
        request_id=args.request_id,
        toolchain=args.toolchain,
        timeout=args.timeout,
    )
    independent = _verify_independent(report, cases)
    ok = report["ok"] and all(check["matches"] for check in independent)
    payload = {
        "request_id": args.request_id,
        "ok": ok,
        "independent": independent,
        "matrix": report,
        "elapsed_seconds": round(time.monotonic() - started, 3),
    }
    text = json.dumps(payload, indent=2)
    if args.output:
        try:
            pathlib.Path(args.output).write_text(text + "\n", encoding="utf-8")
        except OSError as error:
            print(
                f"dependency_helper_witnesses cannot write {args.output} {error}",
                file=sys.stderr,
            )
            return 2
    else:
        print(text)
    mismatches = sorted(
        {check["case_id"] for check in independent if not check["matches"]}
        | {
            check["case_id"]
            for check in report["checks"]
            if not check["matches_expectation"]
        }
    )
    print(
        f"{args.request_id} ok={int(ok)} matrix_checks={len(report['checks'])} "
        f"independent={len(independent)} mismatches={','.join(mismatches) or '-'}"
    )
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
