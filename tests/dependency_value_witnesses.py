"""Bounded compiler witnesses for the M5 primitive value folds."""

import argparse
import hashlib
import json
import pathlib
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from dependency_witnesses import (  # noqa: E402
    DEFAULT_TIMEOUT,
    EDITIONS,
    OVERFLOW_CHECKS,
    OPTIMIZATIONS,
    SIDES,
    run_matrix,
)

U32_MAX = 0xFFFFFFFF
ORIGINAL_IDS = (
    "A07_copy_alias",
    "A08_inline_temporary",
    "A10_tuple_destructure",
    "A18_dead_primitive",
)
ORIGINAL_INPUTS = (0, 1, 15, 16, 31, 255, 65535, U32_MAX)
ORIGINAL_MAIN = (
    "fn main(){for input in [0,1,15,16,31,255,65535,u32::MAX]"
    '{println!("{:?}",f(input));}}'
)
ORIGINAL_MODELS = {
    "A07_copy_alias": lambda input: (input & 15) ^ 3,
    "A08_inline_temporary": lambda input: (input & 15) ^ 3,
    "A10_tuple_destructure": lambda input: (input & 15, input >> 4),
    "A18_dead_primitive": lambda input: input,
}
COMPOSITION_INPUTS = (0, 1, 15, 16, 31, 255, 65535, U32_MAX)
VERSION_INPUTS = (0, 1, 3, 7, 8, 9, 255, U32_MAX)
TUPLE_INPUTS = (0, 1, 7, 8, 15, 16, 255, U32_MAX)
EVALUATION_INPUTS = (0, 1, 3, 7, 8, 15, 16, 255)
TRUTH_INPUTS = (0, 1, 3, 4, 5, 7, 8, 255)


def _debug(value):
    if isinstance(value, tuple):
        return "({0}, {1})".format(*value)
    return str(value)


def _lines(values):
    return "".join(_debug(value) + "\n" for value in values)


def _main(inputs):
    parts = [
        "u32::MAX" if value == U32_MAX else str(value) for value in inputs
    ]
    return (
        "fn main(){for input in ["
        + ",".join(parts)
        + ']{println!("{:?}",f(input));}}'
    )


def _program(fn, inputs):
    return fn + "\n" + _main(inputs)


def _composition_value(input):
    mask = input & 15
    return (mask ^ 3, mask >> 1)


def _multiuse_value(input):
    temporary = input & 7
    return (temporary ^ 1) + (temporary | 2)


def _alias_stable(input):
    base = input & 7
    return (base, base ^ (base ^ 1))


def _alias_erased(input):
    base = input & 7
    return (base, base ^ base)


def _temporary_stable(input):
    base = input & 7
    captured = base + 1
    advanced = base ^ 8
    return captured + (captured + advanced)


def _temporary_erased(input):
    advanced = (input & 7) ^ 8
    return (advanced + 1) + ((advanced + 1) + advanced)


def _storage_values(inputs):
    return [(input & 7) ^ 1 for input in inputs]


V_CASES = [
    {
        "id": "V01_composition",
        "a": _program(
            "fn f(input:u32)->(u32,u32){let mask=input&15u32;"
            "let copied=mask;let temp=copied^3u32;"
            "let (x,y)=(mask|7u32,mask>>1);let dead=17u32;(temp,y)}",
            COMPOSITION_INPUTS,
        ),
        "b": _program(
            "fn f(input:u32)->(u32,u32){let mask=input&15u32;"
            "let x=mask|7u32;let y=mask>>1;(mask^3u32,y)}",
            COMPOSITION_INPUTS,
        ),
        "expected": "same",
        "expect": {
            "a": {
                "exit": 0,
                "stdout": _lines(
                    _composition_value(input)
                    for input in COMPOSITION_INPUTS
                ),
            },
            "b": {
                "exit": 0,
                "stdout": _lines(
                    _composition_value(input)
                    for input in COMPOSITION_INPUTS
                ),
            },
        },
    },
    {
        "id": "V02_multiuse_temporary",
        "a": _program(
            "fn f(input:u32)->u32{let t=input&7u32;"
            "let p=t^1u32;let q=t|2u32;p+q}",
            VERSION_INPUTS,
        ),
        "b": _program(
            "fn f(input:u32)->u32{let p=(input&7u32)^1u32;"
            "let q=(input&7u32)|2u32;p+q}",
            VERSION_INPUTS,
        ),
        "expected": "same",
        "expect": {
            "a": {
                "exit": 0,
                "stdout": _lines(
                    _multiuse_value(input) for input in VERSION_INPUTS
                ),
            },
            "b": {
                "exit": 0,
                "stdout": _lines(
                    _multiuse_value(input) for input in VERSION_INPUTS
                ),
            },
        },
    },
    {
        "id": "V03_alias_reassignment",
        "a": _program(
            "fn f(input:u32)->(u32,u32){let mut base=input&7u32;"
            "let mut copied=base;let first=copied;base=base^1u32;"
            "copied=copied^base;(first,copied)}",
            VERSION_INPUTS,
        ),
        "b": _program(
            "fn f(input:u32)->(u32,u32){let mut base=input&7u32;"
            "let first=base;base=base^1u32;(first,base^base)}",
            VERSION_INPUTS,
        ),
        "expected": "different",
        "expect": {
            "a": {
                "exit": 0,
                "stdout": _lines(
                    _alias_stable(input) for input in VERSION_INPUTS
                ),
            },
            "b": {
                "exit": 0,
                "stdout": _lines(
                    _alias_erased(input) for input in VERSION_INPUTS
                ),
            },
        },
    },
    {
        "id": "V04_temporary_unstable",
        "a": _program(
            "fn f(input:u32)->u32{let mut base=input&7u32;"
            "let t=base+1u32;base=base^8u32;let u=t+base;t+u}",
            VERSION_INPUTS,
        ),
        "b": _program(
            "fn f(input:u32)->u32{let mut base=input&7u32;base=base^8u32;"
            "let u=(base+1u32)+base;let t=base+1u32;t+u}",
            VERSION_INPUTS,
        ),
        "expected": "different",
        "expect": {
            "a": {
                "exit": 0,
                "stdout": _lines(
                    _temporary_stable(input) for input in VERSION_INPUTS
                ),
            },
            "b": {
                "exit": 0,
                "stdout": _lines(
                    _temporary_erased(input) for input in VERSION_INPUTS
                ),
            },
        },
    },
    {
        "id": "V05_tuple_element_effects",
        "a": _program(
            'fn f(input:u32)->u32{let (x,_)=({println!("e");input&7u32},'
            '{println!("d");input>>4});x}',
            TUPLE_INPUTS,
        ),
        "b": _program("fn f(input:u32)->u32{input&7u32}", TUPLE_INPUTS),
        "expected": "different",
        "expect": {
            "a": {
                "exit": 0,
                "stdout": "".join(
                    "e\nd\n" + _debug(input & 7) + "\n"
                    for input in TUPLE_INPUTS
                ),
            },
            "b": {
                "exit": 0,
                "stdout": _lines(input & 7 for input in TUPLE_INPUTS),
            },
        },
    },
    {
        "id": "V06_tuple_drop_order",
        "a": (
            "struct S(u8);impl Drop for S{fn drop(&mut self){"
            'println!("D{}",self.0);}}'
            "fn f()->u8{let (ref x,ref y)=(S(1),S(2));let z=S(9);x.0}"
            'fn main(){println!("{}",f());}'
        ),
        "b": (
            "struct S(u8);impl Drop for S{fn drop(&mut self){"
            'println!("D{}",self.0);}}'
            "fn f()->u8{let x=S(1);let y=S(2);let z=S(9);x.0}"
            'fn main(){println!("{}",f());}'
        ),
        "expected": "different",
        "expect": {
            "a": {"exit": 0, "stdout": "D9\nD1\nD2\n1\n"},
            "b": {"exit": 0, "stdout": "D9\nD2\nD1\n1\n"},
        },
    },
    {
        "id": "V07_dead_initializer_panic",
        "a": _program(
            "fn f(input:u32)->u32{let unused=10u32/input;input}",
            VERSION_INPUTS,
        ),
        "b": _program("fn f(input:u32)->u32{input}", VERSION_INPUTS),
        "expected": "different",
        "expect": {
            "a": {"exit": 101, "stdout": ""},
            "b": {
                "exit": 0,
                "stdout": _lines(input for input in VERSION_INPUTS),
            },
        },
    },
    {
        "id": "V08_alias_storage_observed",
        "a": _program(
            'fn f(input:u32)->u32{let copied=input&7u32;'
            'println!("{}",std::ptr::addr_of!(copied) as *const u32 as '
            "usize);copied^1u32}",
            VERSION_INPUTS,
        ),
        "b": _program(
            "fn f(input:u32)->u32{input&7u32^1u32}", VERSION_INPUTS
        ),
        "expected": "different",
        "expect": {
            "a": {
                "exit": 0,
                "shape": "addr-value",
                "values": _storage_values(VERSION_INPUTS),
            },
            "b": {
                "exit": 0,
                "stdout": _lines(_storage_values(VERSION_INPUTS)),
            },
        },
    },
    {
        "id": "V09_evaluation_neighbor",
        "a": _program(
            "fn f(input:u32)->u32{let t=input&15u32;t^3u32}",
            EVALUATION_INPUTS,
        ),
        "b": _program(
            "fn f(input:u32)->u32{let t=input&15u32;t|3u32}",
            EVALUATION_INPUTS,
        ),
        "expected": "different",
        "expect": {
            "a": {
                "exit": 0,
                "stdout": _lines((input & 15) ^ 3 for input in EVALUATION_INPUTS),
            },
            "b": {
                "exit": 0,
                "stdout": _lines((input & 15) | 3 for input in EVALUATION_INPUTS),
            },
        },
    },
    {
        "id": "V10_truth_neighbor",
        "a": _program(
            "fn f(input:u32)->u32{let flag=input>3u32;"
            "if flag{1u32}else{2u32}}",
            TRUTH_INPUTS,
        ),
        "b": _program(
            "fn f(input:u32)->u32{let flag=input>3u32;"
            "if flag{2u32}else{1u32}}",
            TRUTH_INPUTS,
        ),
        "expected": "different",
        "expect": {
            "a": {
                "exit": 0,
                "stdout": _lines(
                    1 if input > 3 else 2 for input in TRUTH_INPUTS
                ),
            },
            "b": {
                "exit": 0,
                "stdout": _lines(
                    2 if input > 3 else 1 for input in TRUTH_INPUTS
                ),
            },
        },
    },
    {
        "id": "V11_retained_shadow_capture",
        "a": _program(
            "fn f(x:u32)->u32{let saved=x&15u32;let x=opaque(21u32);saved}"
            "fn opaque(y:u32)->u32{y}",
            ORIGINAL_INPUTS,
        ),
        "b": _program(
            "fn f(x:u32)->u32{let x=opaque(21u32);x&15u32}"
            "fn opaque(y:u32)->u32{y}",
            ORIGINAL_INPUTS,
        ),
        "expected": "different",
        "expect": {
            "a": {"exit": 0, "stdout": _lines(value & 15 for value in ORIGINAL_INPUTS)},
            "b": {"exit": 0, "stdout": _lines(5 for _ in ORIGINAL_INPUTS)},
        },
    },
    {
        "id": "V12_ancestor_parameter_raw_storage",
        "a": _program(
            "fn f(mut source:u32)->u32{let pointer=&raw mut source;"
            "{let saved=source;unsafe{*pointer=2u32;}saved}}",
            ORIGINAL_INPUTS,
        ),
        "b": _program(
            "fn f(mut source:u32)->u32{let pointer=&raw mut source;"
            "{unsafe{*pointer=2u32;}source}}",
            ORIGINAL_INPUTS,
        ),
        "expected": "different",
        "expect": {
            "a": {"exit": 0, "stdout": _lines(ORIGINAL_INPUTS)},
            "b": {"exit": 0, "stdout": _lines(2 for _ in ORIGINAL_INPUTS)},
        },
    },
    {
        "id": "V13_ancestor_producer_raw_storage",
        "a": _program(
            "fn f(input:u32)->u32{let mut source=input&15u32;"
            "let pointer=&raw mut source;"
            "{let saved=source;unsafe{*pointer=2u32;}saved}}",
            ORIGINAL_INPUTS,
        ),
        "b": _program(
            "fn f(input:u32)->u32{let mut source=input&15u32;"
            "let pointer=&raw mut source;"
            "{unsafe{*pointer=2u32;}source}}",
            ORIGINAL_INPUTS,
        ),
        "expected": "different",
        "expect": {
            "a": {"exit": 0, "stdout": _lines(value & 15 for value in ORIGINAL_INPUTS)},
            "b": {"exit": 0, "stdout": _lines(2 for _ in ORIGINAL_INPUTS)},
        },
    },
    {
        "id": "V14_ancestor_closure_input_raw_storage",
        "a": _program(
            "fn f(source:u32)->u32{let run=|mut value:u32|{"
            "let pointer=&raw mut value;"
            "{let saved=value;unsafe{*pointer=2u32;}saved}};run(source)}",
            ORIGINAL_INPUTS,
        ),
        "b": _program(
            "fn f(source:u32)->u32{let run=|mut value:u32|{"
            "let pointer=&raw mut value;"
            "{unsafe{*pointer=2u32;}value}};run(source)}",
            ORIGINAL_INPUTS,
        ),
        "expected": "different",
        "expect": {
            "a": {"exit": 0, "stdout": _lines(ORIGINAL_INPUTS)},
            "b": {"exit": 0, "stdout": _lines(2 for _ in ORIGINAL_INPUTS)},
        },
    },
    {
        "id": "V15_combining_mark_format_capture",
        "a": _program(
            'fn f(input:u32)->u32{let x́=input&15u32;'
            'format!("{x́}").parse::<u32>().unwrap()}',
            ORIGINAL_INPUTS,
        ),
        "b": _program(
            'fn f(_input:u32)->u32{let x́=2u32;'
            'format!("{x́}").parse::<u32>().unwrap()}',
            ORIGINAL_INPUTS,
        ),
        "expected": "different",
        "expect": {
            "a": {"exit": 0, "stdout": _lines(value & 15 for value in ORIGINAL_INPUTS)},
            "b": {"exit": 0, "stdout": _lines(2 for _ in ORIGINAL_INPUTS)},
        },
    },
]


def _load_originals(path):
    try:
        cases = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read {path} {error}")
    if not isinstance(cases, list):
        raise ValueError("cases must be a JSON list")
    found = {case.get("id"): case for case in cases if isinstance(case, dict)}
    loaded = []
    for case_id in ORIGINAL_IDS:
        case = found.get(case_id)
        if case is None:
            raise ValueError(f"missing original case {case_id}")
        if case.get("expected") != "same":
            raise ValueError(f"original case {case_id} drifted from same")
        for side in SIDES:
            source = case.get(side)
            if not isinstance(source, str) or ORIGINAL_MAIN not in source:
                raise ValueError(
                    f"original case {case_id} side {side} drifted from "
                    "the studied main loop"
                )
        model = ORIGINAL_MODELS[case_id]
        stdout = _lines(model(input) for input in ORIGINAL_INPUTS)
        loaded.append(
            {
                "id": case_id,
                "a": case["a"],
                "b": case["b"],
                "expected": "same",
                "expect": {
                    "a": {"exit": 0, "stdout": stdout},
                    "b": {"exit": 0, "stdout": stdout},
                },
            }
        )
    return loaded


def _shape_matches(stdout, values):
    lines = stdout.split("\n")
    if len(lines) != 2 * len(values) + 1 or lines[-1] != "":
        return False
    for index, value in enumerate(values):
        if not lines[2 * index].isdigit() or int(lines[2 * index]) < 0:
            return False
        if lines[2 * index + 1] != str(value):
            return False
    return True


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
                        elif matches and "shape" in spec:
                            matches = _shape_matches(
                                run["stdout"], spec["values"]
                            )
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
    parser = argparse.ArgumentParser(
        prog="dependency_value_witnesses",
        description="Run the M5 value fold witness matrix.",
    )
    parser.add_argument(
        "--original-cases",
        default=str(
            pathlib.Path(__file__).resolve().with_name("fixtures")
            .joinpath("dependency", "cases.json")
        ),
        help="fixture path holding the original M5 pairs",
    )
    parser.add_argument("--request-id", default="M5-WITNESS-FUZZ-SLICE-01")
    parser.add_argument("--toolchain", default="stable")
    parser.add_argument(
        "--timeout",
        type=float,
        default=DEFAULT_TIMEOUT,
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
        print("dependency_value_witnesses timeout must be positive", file=sys.stderr)
        return 2
    try:
        cases = _load_originals(args.original_cases)
        cases.extend(V_CASES)
    except ValueError as error:
        print(f"dependency_value_witnesses invalid fixtures {error}", file=sys.stderr)
        return 2
    if args.case:
        known = {case["id"] for case in cases}
        unknown = [value for value in args.case if value not in known]
        if unknown:
            print(
                f"dependency_value_witnesses unknown case ids "
                f"{', '.join(unknown)}",
                file=sys.stderr,
            )
            return 2
        wanted = set(args.case)
        cases = [case for case in cases if case["id"] in wanted]
        if not cases:
            print("dependency_value_witnesses case filter removed all cases", file=sys.stderr)
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
                f"dependency_value_witnesses cannot write {args.output} {error}",
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
