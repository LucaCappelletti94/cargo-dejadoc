"""Bounded compiler and runtime witnesses for the M8 item ordering folds."""

import argparse
import concurrent.futures
import hashlib
import json
import pathlib
import sys
import tempfile
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from dependency_witnesses import (  # noqa: E402
    COMPILE_CAP,
    EDITIONS,
    OVERFLOW_CHECKS,
    OPTIMIZATIONS,
    RUN_CAP,
    SIDES,
    WORKERS,
    run_matrix,
    run_process,
)
from dependency_structure_witnesses import (  # noqa: E402
    _verify_categories,
    _verify_independent,
    run_categories,
)

ORIGINAL_IDS = ("A13_item_order", "A14_split_impl")
ORIGINAL_EXPECTS = {
    "A13_item_order": {
        "exit": 0,
        "stdout": "(1, 2)\n",
    },
    "A14_split_impl": {
        "exit": 0,
        "stdout": "(1, 2)\n",
    },
}
RELATIONS = frozenset(("same", "different"))
MACRO_SOURCES = {
    "itemobs": pathlib.Path(__file__).resolve().with_name(
        "fixtures").joinpath("itemobserver", "itemobs.rs"),
    "observer": pathlib.Path(__file__).resolve().with_name(
        "fixtures").joinpath("itemobserver", "itemobs.rs"),
}
DEFAULT_TIMEOUT = 1800.0
DEFAULT_CASES = (
    pathlib.Path(__file__).resolve().with_name("fixtures").joinpath(
        "dependency", "item_cases.json")
)
ORIGINAL_CASES = (
    pathlib.Path(__file__).resolve().with_name("fixtures").joinpath(
        "dependency", "cases.json")
)


def _load_originals(path):
    try:
        cases = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read {path} {error}")
    if not isinstance(cases, list):
        raise ValueError("original cases must be a JSON list")
    found = {case.get("id"): case for case in cases if isinstance(case, dict)}
    loaded = []
    for case_id in ORIGINAL_IDS:
        case = found.get(case_id)
        if case is None:
            raise ValueError(f"missing original case {case_id}")
        if case.get("expected") != "same":
            raise ValueError(f"original case {case_id} drifted from same")
        for side in SIDES:
            if not isinstance(case.get(side), str) or not case.get(side):
                raise ValueError(f"original case {case_id} side {side} drifted")
        expect = ORIGINAL_EXPECTS[case_id]
        loaded.append(
            {
                "id": case_id,
                "a": case["a"],
                "b": case["b"],
                "expected": "same",
                "expect": {"a": dict(expect), "b": dict(expect)},
            }
        )
    return loaded


def _load_native(path):
    try:
        cases = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read {path} {error}")
    if not isinstance(cases, list) or not cases:
        raise ValueError("native cases must be a nonempty JSON list")
    runtime = []
    categories = []
    seen = set()
    for position, case in enumerate(cases):
        if not isinstance(case, dict):
            raise ValueError(f"case at position {position} is not an object")
        case_id = case.get("id")
        if not isinstance(case_id, str) or not case_id or case_id in seen:
            raise ValueError(f"case at position {position} has a bad id")
        if any(part in case_id for part in ("/", "\\", "\x00", "..")):
            raise ValueError(f"native case {case_id} has an unusable id")
        seen.add(case_id)
        if not isinstance(case.get("a"), str) or not isinstance(case.get("b"), str):
            raise ValueError(f"native case {case_id} is missing a source")
        externs = case.get("externs", [])
        if not isinstance(externs, list) or any(
                not isinstance(name, str) or name not in MACRO_SOURCES
                for name in externs):
            raise ValueError(f"native case {case_id} has unsupported externs")
        cfg = case.get("cfg", [])
        if not isinstance(cfg, list) or any(
                not isinstance(flag, str) or not flag for flag in cfg):
            raise ValueError(f"native case {case_id} has unsupported cfg flags")
        expect = case.get("expect")
        if not isinstance(expect, dict):
            raise ValueError(f"native case {case_id} has no expect object")
        if "compile" in expect.get("a", {}) or "compile" in expect.get("b", {}):
            for side in SIDES:
                spec = expect.get(side)
                if (
                    not isinstance(spec, dict)
                    or not isinstance(spec.get("compile"), list)
                    or not spec["compile"]
                    or not all(isinstance(tag, str) and tag for tag in spec["compile"])
                    or spec.get("run") != "absent"
                    or "exit" in spec
                    or "stdout" in spec
                ):
                    raise ValueError(
                        f"native case {case_id} side {side} has a bad category spec"
                    )
            categories.append(
                {
                    "id": case_id,
                    "a": case["a"],
                    "b": case["b"],
                    "expect": expect,
                }
            )
        else:
            expected = case.get("expected")
            if expected not in RELATIONS:
                raise ValueError(
                    f"native case {case_id} has unsupported relation {expected!r}"
                )
            side_expect = {}
            for side in SIDES:
                spec = expect.get(side)
                if (
                    not isinstance(spec, dict)
                    or not isinstance(spec.get("exit"), int)
                    or not isinstance(spec.get("stdout"), str)
                ):
                    raise ValueError(
                        f"native case {case_id} side {side} needs an exit and a stdout"
                    )
                if "stderr" in spec and not isinstance(spec["stderr"], str):
                    raise ValueError(
                        f"native case {case_id} side {side} has a bad stderr substring"
                    )
                side_expect[side] = spec
            runtime.append(
                {
                    "id": case_id,
                    "sources": {side: case[side] for side in SIDES},
                    "expected": expected,
                    "expect": side_expect,
                    "externs": list(externs),
                    "cfg": list(cfg),
                }
            )
    return runtime, categories


def _build_observers(scratch_root, request_id, toolchain, deadline):
    records = []
    for name in sorted(MACRO_SOURCES):
        output = str(scratch_root / f"lib{name}.so")
        record = run_process(
            [
                "rustc",
                f"+{toolchain}",
                "--edition",
                "2021",
                "--crate-type",
                "proc-macro",
                "--crate-name",
                name,
                "--extern",
                "proc_macro",
                str(MACRO_SOURCES[name]),
                "-o",
                output,
                "-C",
                "opt-level=2",
            ],
            request_id=f"{request_id}/{name}-macro",
            deadline=deadline,
            timeout=COMPILE_CAP,
        )
        records.append({"name": name, "output": output, "record": record})
    return records, {entry["name"]: entry["output"] for entry in records}


def _run_native_job(scratch_root, request_id, toolchain, deadline, case,
                    macro_paths, job):
    case_id, edition, optimization, overflow_checks, side = job
    row_id = f"{request_id}/{case_id}/{edition}/{optimization}/{int(overflow_checks)}/{side}"
    source = case["sources"][side]
    folder = (
        scratch_root
        / case_id
        / edition
        / str(optimization)
        / str(int(overflow_checks))
        / side
    )
    folder.mkdir(parents=True)
    source_path = folder / "main.rs"
    source_path.write_text(source, encoding="utf-8")
    binary_path = folder / "witness"
    command = [
        "rustc",
        f"+{toolchain}",
        "--edition",
        edition,
        str(source_path),
        "-o",
        str(binary_path),
        "-C",
        f"opt-level={optimization}",
        "-C",
        f"overflow-checks={'yes' if overflow_checks else 'no'}",
        "-C",
        "codegen-units=1",
    ]
    for flag in case["cfg"]:
        command += ["--cfg", flag]
    for name in case["externs"]:
        command += ["--extern", f"{name}={macro_paths[name]}"]
    compile_record = run_process(
        command,
        request_id=row_id + "/compile",
        deadline=deadline,
        timeout=COMPILE_CAP,
    )
    run_record = None
    if compile_record["exit"] == 0:
        run_record = run_process(
            [str(binary_path)],
            request_id=row_id + "/run",
            deadline=deadline,
            timeout=RUN_CAP,
        )
    return {
        "request_id": row_id,
        "side": side,
        "edition": edition,
        "optimization": optimization,
        "overflow_checks": overflow_checks,
        "sha256": hashlib.sha256(source.encode("utf-8")).hexdigest(),
        "compile": compile_record,
        "run": run_record,
    }


def _run_native_jobs(scratch_root, cases, request_id, toolchain, deadline,
                     macro_paths):
    """Run every native case side under every configuration in one deadline."""
    case_lookup = {case["id"]: case for case in cases}
    jobs = []
    for case in cases:
        for edition in EDITIONS:
            for optimization in OPTIMIZATIONS:
                for overflow_checks in OVERFLOW_CHECKS:
                    for side in SIDES:
                        jobs.append((case["id"], edition, optimization,
                                     overflow_checks, side))

    def row_id_for(job):
        case_id, edition, optimization, overflow_checks, side = job
        return f"{request_id}/{case_id}/{edition}/{optimization}/{int(overflow_checks)}/{side}"

    def failed_result(job, reason):
        case_id, edition, optimization, overflow_checks, side = job
        source = case_lookup[case_id]["sources"][side]
        return {
            "request_id": row_id_for(job),
            "side": side,
            "edition": edition,
            "optimization": optimization,
            "overflow_checks": overflow_checks,
            "sha256": hashlib.sha256(source.encode("utf-8")).hexdigest(),
            "compile": {
                "request_id": row_id_for(job) + "/compile",
                "exit": None,
                "stdout": "",
                "stderr": reason,
                "timed_out": True,
            },
            "run": None,
        }

    results = []
    expected_ids = {row_id_for(job) for job in jobs}
    executor = concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS)
    pending = {}
    remaining_jobs = iter(jobs)

    def submit_next():
        if time.monotonic() >= deadline:
            return
        job = next(remaining_jobs, None)
        if job is not None:
            future = executor.submit(
                _run_native_job,
                scratch_root,
                request_id,
                toolchain,
                deadline,
                case_lookup[job[0]],
                macro_paths,
                job,
            )
            pending[future] = job

    def collect(future, job, bound):
        try:
            row = future.result(timeout=bound)
            if row["request_id"] != row_id_for(job):
                row = failed_result(job, "worker returned an unrelated request id")
        except Exception as error:
            row = failed_result(job, f"worker error {error}")
        results.append(row)

    try:
        for _ in range(WORKERS):
            submit_next()
        while pending:
            completed, _ = concurrent.futures.wait(
                pending,
                timeout=max(0.0, deadline - time.monotonic()),
                return_when=concurrent.futures.FIRST_COMPLETED,
            )
            if not completed:
                break
            for future in completed:
                collect(future, pending.pop(future), 0.0)
                submit_next()
    finally:
        executor.shutdown(wait=False, cancel_futures=True)
        for future, job in pending.items():
            collect(future, job, max(0.0, deadline - time.monotonic()))
    for job in remaining_jobs:
        results.append(failed_result(job, "job not started before the deadline"))
    lookup = {}
    duplicates = []
    for row in results:
        if row["request_id"] in lookup:
            duplicates.append(row["request_id"])
        lookup[row["request_id"]] = row
    missing = sorted(expected_ids - set(lookup))
    order = {row_id: index for index, row_id in enumerate(sorted(expected_ids))}
    results.sort(key=lambda row: order.get(row["request_id"], len(order)))
    return results, missing, duplicates


def _verify_relations(request_id, cases, rows):
    lookup = {row["request_id"]: row for row in rows}
    checks = []
    for case in cases:
        case_id = case["id"]
        for edition in EDITIONS:
            for optimization in OPTIMIZATIONS:
                for overflow_checks in OVERFLOW_CHECKS:
                    prefix = (
                        f"{request_id}/{case_id}/{edition}/{optimization}"
                        f"/{int(overflow_checks)}"
                    )
                    row_a = lookup.get(prefix + "/a")
                    row_b = lookup.get(prefix + "/b")
                    complete = (
                        row_a is not None
                        and row_b is not None
                        and not row_a["compile"]["timed_out"]
                        and not row_b["compile"]["timed_out"]
                        and row_a["run"] is not None
                        and row_b["run"] is not None
                        and not row_a["run"]["timed_out"]
                        and not row_b["run"]["timed_out"]
                    )
                    compiled = (
                        complete
                        and row_a["compile"]["exit"] == 0
                        and row_b["compile"]["exit"] == 0
                    )
                    equal = (
                        compiled
                        and (row_a["run"]["exit"], row_a["run"]["stdout"])
                        == (row_b["run"]["exit"], row_b["run"]["stdout"])
                    )
                    if case["expected"] == "same":
                        matches = compiled and equal
                    else:
                        matches = compiled and not equal
                    checks.append(
                        {
                            "case_id": case_id,
                            "expected": case["expected"],
                            "edition": edition,
                            "optimization": optimization,
                            "overflow_checks": overflow_checks,
                            "compiled": compiled,
                            "equal": equal,
                            "matches": matches,
                            "a_request_id": prefix + "/a",
                            "b_request_id": prefix + "/b",
                        }
                    )
    return checks


def main(argv=None):
    parser = argparse.ArgumentParser(
        prog="dependency_item_witnesses",
        description="Run the M8 item ordering and inherent partition witnesses.",
    )
    parser.add_argument(
        "--cases",
        default=str(DEFAULT_CASES),
        help="fixture path holding the native source pairs",
    )
    parser.add_argument(
        "--original-cases",
        default=str(ORIGINAL_CASES),
        help="fixture path holding the original A13 and A14 pairs",
    )
    parser.add_argument("--request-id", default="M8-ITEM-WITNESS-01")
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
        print("dependency_item_witnesses timeout must be positive", file=sys.stderr)
        return 2
    try:
        originals = _load_originals(args.original_cases)
        runtime, categories = _load_native(args.cases)
    except ValueError as error:
        print(f"dependency_item_witnesses invalid fixtures {error}", file=sys.stderr)
        return 2
    if args.case:
        known = (
            {case["id"] for case in originals}
            | {case["id"] for case in runtime}
            | {case["id"] for case in categories}
        )
        unknown = [value for value in args.case if value not in known]
        if unknown:
            print(
                f"dependency_item_witnesses unknown case ids {', '.join(unknown)}",
                file=sys.stderr,
            )
            return 2
        wanted = set(args.case)
        originals = [case for case in originals if case["id"] in wanted]
        runtime = [case for case in runtime if case["id"] in wanted]
        categories = [case for case in categories if case["id"] in wanted]
        if not (originals or runtime or categories):
            print(
                "dependency_item_witnesses case filter removed all cases",
                file=sys.stderr,
            )
            return 2
    started = time.monotonic()
    deadline = started + args.timeout
    if originals:
        try:
            report = run_matrix(
                originals,
                request_id=args.request_id,
                toolchain=args.toolchain,
                timeout=args.timeout,
            )
        except ValueError as error:
            print(
                f"dependency_item_witnesses invalid matrix fixtures {error}",
                file=sys.stderr,
            )
            return 2
        independent = _verify_independent(report, originals)
    else:
        report = {
            "request_id": args.request_id,
            "ok": True,
            "compiler": None,
            "results": [],
            "checks": [],
            "error": None,
            "elapsed_seconds": 0.0,
        }
        independent = []
    native_rows, missing, duplicates = [], [], []
    observers = []
    macro_paths = {}
    if runtime:
        with tempfile.TemporaryDirectory(prefix="echoes-item-witness-") as scratch:
            scratch_root = pathlib.Path(scratch)
            if any(case["externs"] for case in runtime):
                observers, macro_paths = _build_observers(
                    scratch_root, args.request_id, args.toolchain, deadline
                )
            native_rows, missing, duplicates = _run_native_jobs(
                scratch_root,
                runtime,
                args.request_id,
                args.toolchain,
                deadline,
                macro_paths,
            )
    native_independent = _verify_independent(
        {"request_id": args.request_id, "results": native_rows}, runtime
    )
    relations = _verify_relations(args.request_id, runtime, native_rows)
    category_rows, c_missing, c_duplicates = run_categories(
        categories,
        request_id=args.request_id,
        toolchain=args.toolchain,
        deadline=deadline,
    )
    category_checks, c_missing, c_duplicates = _verify_categories(
        args.request_id, categories, category_rows, c_missing, c_duplicates
    )
    problem = None
    if missing:
        problem = f"missing native results {len(missing)} for {args.request_id}"
    elif duplicates:
        problem = f"duplicate native results {len(duplicates)} for {args.request_id}"
    if c_missing:
        problem = problem or (
            f"missing category results {len(c_missing)} for {args.request_id}"
        )
    elif c_duplicates:
        problem = problem or (
            f"duplicate category results {len(c_duplicates)} for {args.request_id}"
        )
    macro_ok = all(
        entry["record"]["exit"] == 0 and not entry["record"]["timed_out"]
        for entry in observers
    )
    ok = (
        report["ok"]
        and problem is None
        and macro_ok
        and all(check["matches"] for check in independent)
        and all(check["matches"] for check in native_independent)
        and all(check["matches"] for check in relations)
        and all(check["matches"] for check in category_checks)
    )
    payload = {
        "request_id": args.request_id,
        "ok": ok,
        "original_sources": {
            case["id"]: {
                "a_sha256": hashlib.sha256(case["a"].encode("utf-8")).hexdigest(),
                "b_sha256": hashlib.sha256(case["b"].encode("utf-8")).hexdigest(),
                "expected": case["expected"],
            }
            for case in originals
        },
        "observers": observers,
        "native_rows": native_rows,
        "independent": independent + native_independent,
        "relations": relations,
        "categories": category_checks,
        "category_rows": category_rows,
        "matrix": report,
        "error": problem,
        "elapsed_seconds": round(time.monotonic() - started, 3),
    }
    text = json.dumps(payload, indent=2)
    if args.output:
        try:
            pathlib.Path(args.output).write_text(text + "\n", encoding="utf-8")
        except OSError as error:
            print(
                f"dependency_item_witnesses cannot write {args.output} {error}",
                file=sys.stderr,
            )
            return 2
    else:
        print(text)
    mismatches = sorted(
        {check["case_id"] for check in independent if not check["matches"]}
        | {check["case_id"] for check in native_independent if not check["matches"]}
        | {check["case_id"] for check in relations if not check["matches"]}
        | {check["case_id"] for check in category_checks if not check["matches"]}
        | {check["case_id"] for check in report["checks"]
           if not check["matches_expectation"]}
    )
    print(
        f"{args.request_id} ok={int(ok)} matrix_checks={len(report['checks'])} "
        f"independent={len(independent) + len(native_independent)} "
        f"relations={len(relations)} categories={len(category_checks)} "
        f"mismatches={','.join(mismatches) or '-'}"
    )
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
