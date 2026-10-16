"""Bounded compiler witnesses for the M7 named-field and literal-alternative folds."""

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
    SIDES,
    WORKERS,
    run_matrix,
    run_process,
)

ORIGINAL_IDS = ("A15_literal_alternatives", "A17_field_order")
INPUTS = (0, 1, 15, 16, 31, 255, 65535, 0xFFFFFFFF)
DEFAULT_TIMEOUT = 1800.0
DEFAULT_CASES = (
    pathlib.Path(__file__).resolve().with_name("fixtures").joinpath("dependency", "structure_cases.json")
)
ORIGINAL_CASES = (
    pathlib.Path(__file__).resolve().with_name("fixtures").joinpath("dependency", "cases.json")
)
def _debug(value):
    if isinstance(value, tuple):
        return "({0}, {1})".format(*value)
    if isinstance(value, bool):
        return "true" if value else "false"
    return str(value)


def _lines(values):
    return "".join(_debug(value) + "\n" for value in values)


def _pair(input):
    return (input & 15, input >> 4)


ORIGINAL_EXPECTS = {
    "A15_literal_alternatives": {
        "exit": 0,
        "stdout": _lines([i in (1, 2) for i in INPUTS]),
    },
    "A17_field_order": {
        "exit": 0,
        "stdout": _lines(_pair(i) for i in INPUTS),
    },
}


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
    for case in cases:
        if not isinstance(case, dict):
            raise ValueError("native case is not an object")
        case_id = case.get("id")
        if not isinstance(case_id, str) or not case_id or case_id in seen:
            raise ValueError(f"native case {case_id!r} has a bad or duplicate id")
        seen.add(case_id)
        if not isinstance(case.get("a"), str) or not isinstance(case.get("b"), str):
            raise ValueError(f"native case {case_id} is missing a source")
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
            categories.append(case)
        else:
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
            runtime.append(case)
    return runtime, categories


def _verify_independent(report, cases):
    lookup = {row["request_id"]: row for row in report["results"]}
    checks = []
    for case in cases:
        case_id = case["id"]
        for edition in EDITIONS:
            for optimization in OPTIMIZATIONS:
                for overflow_checks in OVERFLOW_CHECKS:
                    for side in SIDES:
                        spec = case["expect"][side]
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
                            and run["stdout"] == spec["stdout"]
                        )
                        if matches and "stderr" in spec:
                            matches = spec["stderr"] in (run["stderr"] or "")
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
                                "expected_stdout_sha256": hashlib.sha256(
                                    spec["stdout"].encode("utf-8")
                                ).hexdigest(),
                                "stdout_sha256": hashlib.sha256(
                                    (run["stdout"] if run else "").encode("utf-8")
                                ).hexdigest(),
                                "matches": matches,
                            }
                        )
    return checks


def _run_category_job(scratch_root, request_id, toolchain, deadline, sources, job):
    case_id, edition, optimization, overflow_checks, side = job
    row_id = f"{request_id}/{case_id}/{edition}/{optimization}/{int(overflow_checks)}/{side}/compile"
    source = sources[case_id][0] if side == "a" else sources[case_id][1]
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
    command = [
        "rustc",
        f"+{toolchain}",
        "--edition",
        edition,
        str(source_path),
        "-o",
        str(folder / "witness"),
        "-C",
        f"opt-level={optimization}",
        "-C",
        f"overflow-checks={'yes' if overflow_checks else 'no'}",
        "-C",
        "codegen-units=1",
    ]
    record = run_process(
        command,
        request_id=row_id,
        deadline=deadline,
        timeout=COMPILE_CAP,
    )
    return {
        "request_id": row_id,
        "side": side,
        "edition": edition,
        "optimization": optimization,
        "overflow_checks": overflow_checks,
        "compile": record,
    }


def run_categories(categories, *, request_id, toolchain, deadline):
    """Verify every category case side under every configuration within one deadline."""
    sources = {case["id"]: (case["a"], case["b"]) for case in categories}
    jobs = []
    for case in categories:
        for edition in EDITIONS:
            for optimization in OPTIMIZATIONS:
                for overflow_checks in OVERFLOW_CHECKS:
                    for side in SIDES:
                        jobs.append((case["id"], edition, optimization, overflow_checks, side))

    def row_id_for(job):
        case_id, edition, optimization, overflow_checks, side = job
        return f"{request_id}/{case_id}/{edition}/{optimization}/{int(overflow_checks)}/{side}/compile"

    def failed_result(job, reason):
        case_id, edition, optimization, overflow_checks, side = job
        return {
            "request_id": row_id_for(job),
            "side": side,
            "edition": edition,
            "optimization": optimization,
            "overflow_checks": overflow_checks,
            "compile": {
                "request_id": row_id_for(job),
                "exit": None,
                "stdout": "",
                "stderr": reason,
                "timed_out": True,
            },
        }

    results = []
    expected_ids = {row_id_for(job) for job in jobs}
    with tempfile.TemporaryDirectory(prefix="dejadoc-structure-category-") as scratch:
        scratch_root = pathlib.Path(scratch)
        executor = concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS)
        pending = {}
        remaining_jobs = iter(jobs)

        def submit_next():
            if time.monotonic() >= deadline:
                return
            job = next(remaining_jobs, None)
            if job is not None:
                future = executor.submit(
                    _run_category_job,
                    scratch_root,
                    request_id,
                    toolchain,
                    deadline,
                    sources,
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


def _verify_categories(request_id, categories, rows, missing, duplicates):
    lookup = {row["request_id"]: row for row in rows}
    checks = []
    for case in categories:
        case_id = case["id"]
        for edition in EDITIONS:
            for optimization in OPTIMIZATIONS:
                for overflow_checks in OVERFLOW_CHECKS:
                    for side in SIDES:
                        spec = case["expect"][side]
                        row_id = (
                            f"{request_id}/{case_id}/{edition}"
                            f"/{optimization}/{int(overflow_checks)}/{side}/compile"
                        )
                        row = lookup.get(row_id)
                        record = row["compile"] if row else None
                        matches = (
                            record is not None
                            and record["exit"] not in (0, None)
                            and not record["timed_out"]
                            and all(
                                tag in (record["stderr"] or "") for tag in spec["compile"]
                            )
                        )
                        checks.append(
                            {
                                "case_id": case_id,
                                "side": side,
                                "edition": edition,
                                "optimization": optimization,
                                "overflow_checks": overflow_checks,
                                "request_id": row_id,
                                "categories": spec["compile"],
                                "exit": record["exit"] if record else None,
                                "matches": matches,
                            }
                        )
    return checks, missing, duplicates


def main(argv=None):
    parser = argparse.ArgumentParser(
        prog="dependency_structure_witnesses",
        description="Run the M7 structure fold witness matrix.",
    )
    parser.add_argument(
        "--cases",
        default=str(DEFAULT_CASES),
        help="fixture path holding the native source pairs",
    )
    parser.add_argument(
        "--original-cases",
        default=str(ORIGINAL_CASES),
        help="fixture path holding the original M7 pairs",
    )
    parser.add_argument("--request-id", default="M7-NATIVE-SURFACE-01")
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
        print("dependency_structure_witnesses timeout must be positive", file=sys.stderr)
        return 2
    try:
        originals = _load_originals(args.original_cases)
        runtime, categories = _load_native(args.cases)
    except ValueError as error:
        print(f"dependency_structure_witnesses invalid fixtures {error}", file=sys.stderr)
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
                f"dependency_structure_witnesses unknown case ids {', '.join(unknown)}",
                file=sys.stderr,
            )
            return 2
        wanted = set(args.case)
        originals = [case for case in originals if case["id"] in wanted]
        runtime = [case for case in runtime if case["id"] in wanted]
        categories = [case for case in categories if case["id"] in wanted]
        if not (originals or runtime or categories):
            print(
                "dependency_structure_witnesses case filter removed all cases",
                file=sys.stderr,
            )
            return 2
    started = time.monotonic()
    deadline = started + args.timeout
    try:
        report = run_matrix(
            originals + runtime,
            request_id=args.request_id,
            toolchain=args.toolchain,
            timeout=args.timeout,
        )
    except ValueError as error:
        print(
            f"dependency_structure_witnesses invalid matrix fixtures {error}",
            file=sys.stderr,
        )
        return 2
    independent = _verify_independent(report, originals + runtime)
    category_rows, missing, duplicates = run_categories(
        categories,
        request_id=args.request_id,
        toolchain=args.toolchain,
        deadline=deadline,
    )
    category_checks, missing, duplicates = _verify_categories(
        args.request_id, categories, category_rows, missing, duplicates
    )
    problem = None
    if missing:
        problem = f"missing category results {len(missing)} for {args.request_id}"
    elif duplicates:
        problem = f"duplicate category results {len(duplicates)} for {args.request_id}"
    ok = (
        report["ok"]
        and problem is None
        and all(check["matches"] for check in independent)
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
        "independent": independent,
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
                f"dependency_structure_witnesses cannot write {args.output} {error}",
                file=sys.stderr,
            )
            return 2
    else:
        print(text)
    mismatches = sorted(
        {check["case_id"] for check in independent if not check["matches"]}
        | {check["case_id"] for check in category_checks if not check["matches"]}
        | {check["case_id"] for check in report["checks"] if not check["matches_expectation"]}
    )
    print(
        f"{args.request_id} ok={int(ok)} matrix_checks={len(report['checks'])} "
        f"independent={len(independent)} categories={len(category_checks)} "
        f"mismatches={','.join(mismatches) or '-'}"
    )
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
