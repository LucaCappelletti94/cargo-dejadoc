"""Bounded compiler and runtime verification for checked fixed-width constants."""

import argparse
import concurrent.futures
import hashlib
import json
import math
import re
import pathlib
import sys
import tempfile
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from dependency_witnesses import (  # noqa: E402
    COMPILE_CAP,
    EDITIONS,
    GRACE_SECONDS,
    OVERFLOW_CHECKS,
    OPTIMIZATIONS,
    PROBE_CAP,
    RUN_CAP,
    SIDES,
    WORKERS,
    run_process,
)

RELATIONS = frozenset(("same", "different", "profile-dependent"))
RUN_CLASSES = frozenset(("value", "diverge", "absent"))
OVERFLOW_KEYS = ("off", "on")
MACRO_SOURCES = {
    "obs": pathlib.Path(__file__).resolve().with_name(
        "fixtures").joinpath("dependency", "obs_attr.rs"),
}
DEFAULT_TIMEOUT = 1800.0
DEFAULT_CASES = (
    pathlib.Path(__file__).resolve().with_name("fixtures").joinpath(
        "dependency", "constant_cases.json")
)


def _validate_case(position, case):
    case_id = case.get("id")
    if not isinstance(case_id, str) or not case_id:
        raise ValueError(f"case at position {position} has no id")
    if any(part in case_id for part in ("/", "\\", "\x00", "..")):
        raise ValueError(f"case {case_id} has an unusable id")
    family = case.get("family")
    if not isinstance(family, str) or not family:
        raise ValueError(f"case {case_id} has no family")
    sources = {}
    for side in SIDES:
        source = case.get(side)
        if not isinstance(source, str) or not source:
            raise ValueError(f"case {case_id} is missing source {side}")
        sources[side] = source
    expected = case.get("expected")
    if not isinstance(expected, str) or expected not in RELATIONS:
        raise ValueError(f"case {case_id} has unsupported relation {expected!r}")
    expectations = {}
    expect = case.get("expect")
    if not isinstance(expect, dict):
        raise ValueError(f"case {case_id} has no expectations")
    for side in SIDES:
        side_expect = expect.get(side)
        if not isinstance(side_expect, dict):
            raise ValueError(f"case {case_id} has no expectations for {side}")
        for key in OVERFLOW_KEYS:
            entry = side_expect.get(key)
            if not isinstance(entry, dict):
                raise ValueError(
                    f"case {case_id} side {side} has no {key} expectation")
            _validate_side_expectation(case_id, side, key, entry)
        expectations[side] = side_expect
    externs = case.get("externs", [])
    if not isinstance(externs, list) or any(
            not isinstance(name, str) or name not in MACRO_SOURCES
            for name in externs):
        raise ValueError(f"case {case_id} has unsupported externs {externs!r}")
    return {
        "id": case_id,
        "family": family,
        "sources": sources,
        "expected": expected,
        "expect": expectations,
        "externs": externs,
    }


def _validate_side_expectation(case_id, side, key, entry):
    compile_expect = entry.get("compile")
    if compile_expect == "ok":
        pass
    elif isinstance(compile_expect, list) and compile_expect and all(
            isinstance(text, str) and text for text in compile_expect):
        pass
    else:
        raise ValueError(
            f"case {case_id} side {side} {key} has no usable compile expectation")
    run_class = entry.get("run")
    if run_class not in RUN_CLASSES:
        raise ValueError(
            f"case {case_id} side {side} {key} has unsupported run class {run_class!r}")
    if run_class == "absent" and compile_expect == "ok":
        raise ValueError(
            f"case {case_id} side {side} {key} expects a run of a missing side")
    if run_class in ("value", "diverge"):
        stdout = entry.get("stdout")
        if not isinstance(stdout, str):
            raise ValueError(
                f"case {case_id} side {side} {key} is missing the stdout literal")
    if run_class == "diverge":
        exit_code = entry.get("exit")
        if not isinstance(exit_code, int) or not 0 < exit_code < 256:
            raise ValueError(
                f"case {case_id} side {side} {key} has no usable divergent exit code")
    if run_class == "value" and "exit" in entry:
        raise ValueError(
            f"case {case_id} side {side} {key} carries an exit code in value class")
    if "stderr" in entry and not isinstance(entry.get("stderr"), str):
        raise ValueError(
            f"case {case_id} side {side} {key} has no usable stderr substring")


def _validate_cases(cases):
    if not isinstance(cases, (list, tuple)) or not cases:
        raise ValueError("cases must be a nonempty list of objects")
    validated = []
    seen = set()
    for position, case in enumerate(cases):
        if not isinstance(case, dict):
            raise ValueError(f"case at position {position} is not an object")
        entry = _validate_case(position, case)
        if entry["id"] in seen:
            raise ValueError(f"duplicate case id {entry['id']}")
        seen.add(entry["id"])
        validated.append(entry)
    return validated


def _normalize_text(text, scratch_root):
    """Strip pointer lines and witness paths from diagnostic text."""
    text = text.replace(str(scratch_root), "<witness>")
    lines = [
        line for line in text.splitlines()
        if not line.lstrip().startswith("-->")
        and not ("panicked at " in line and line.lstrip().startswith("thread "))
    ]
    return "\n".join(lines)


def _run_job(scratch_root, request_id, toolchain, deadline, case, externs,
             edition, optimization, overflow_checks, side):
    case_id = case["id"]
    row_id = (
        f"{request_id}/{case_id}/{edition}/{optimization}"
        f"/{int(overflow_checks)}/{side}"
    )
    source = case["sources"][side]
    folder = (
        scratch_root / case_id / edition / str(optimization)
        / str(int(overflow_checks)) / side
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
        *([f"--extern", f"obs={scratch_root / 'libobs.so'}"] if externs else []),
    ]
    compile_record = run_process(
        command,
        request_id=row_id + "/compile",
        deadline=deadline,
        timeout=COMPILE_CAP,
    )
    compile_record["norm_stderr"] = _normalize_text(
        compile_record["stderr"], scratch_root)
    run_record = None
    if compile_record["exit"] == 0:
        run_record = run_process(
            [str(binary_path)],
            request_id=row_id + "/run",
            deadline=deadline,
            timeout=RUN_CAP,
        )
        run_record["norm_stderr"] = _normalize_text(
            run_record["stderr"], scratch_root)
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


def _observation(row):
    """The correlated observation key used for the pairwise relation."""
    if row is None or row["compile"]["exit"] != 0:
        stderr = (
            row["compile"]["norm_stderr"] if row is not None else "missing row"
        )
        categories = re.findall(r"error\[([A-Z][0-9]+)\]", stderr)
        categories.extend(name for name in (
            "arithmetic_overflow", "unconditional_panic", "overflowing_literals"
        ) if name in stderr)
        return ("compile", tuple(sorted(set(categories))))
    run = row["run"]
    if run is None or run["exit"] is None:
        return ("incomplete",)
    return ("run", run["exit"], run["stdout"], run["norm_stderr"])


def _side_matches(row, expectation, case_id, side, key):
    problems = []
    compile_expect = expectation["compile"]
    compiled = row is not None and row["compile"]["exit"] == 0 \
        and not row["compile"]["timed_out"]
    if compiled:
        if compile_expect != "ok":
            problems.append(f"{case_id}/{side}/{key} compiled against a "
                            f"compile-error expectation")
    else:
        if compile_expect == "ok":
            problems.append(f"{case_id}/{side}/{key} failed to compile")
        else:
            stderr = row["compile"]["stderr"] if row is not None else ""
            for needle in compile_expect:
                if needle not in stderr:
                    problems.append(
                        f"{case_id}/{side}/{key} stderr lacks {needle!r}")
    run_class = expectation["run"]
    run = row["run"] if row is not None else None
    if run_class == "absent":
        if row is not None and row["compile"]["exit"] == 0:
            problems.append(f"{case_id}/{side}/{key} ran but must not run")
        return problems
    if run is None or run["exit"] is None:
        problems.append(f"{case_id}/{side}/{key} has no correlated run result")
        return problems
    if run["timed_out"]:
        problems.append(f"{case_id}/{side}/{key} run timed out")
        return problems
    if run_class == "value":
        if run["exit"] != 0:
            problems.append(
                f"{case_id}/{side}/{key} exited {run['exit']} instead of 0")
        elif run["stdout"] != expectation["stdout"]:
            problems.append(
                f"{case_id}/{side}/{key} printed {run['stdout']!r} instead "
                f"of {expectation['stdout']!r}")
    elif run_class == "diverge":
        if run["exit"] != expectation["exit"]:
            problems.append(
                f"{case_id}/{side}/{key} exited {run['exit']} instead of "
                f"{expectation['exit']}")
        if run["stdout"] != expectation["stdout"]:
            problems.append(
                f"{case_id}/{side}/{key} printed {run['stdout']!r} instead "
                f"of {expectation['stdout']!r}")
        stderr = expectation.get("stderr")
        if stderr is not None and stderr not in run["stderr"]:
            problems.append(
                f"{case_id}/{side}/{key} stderr lacks {stderr!r}")
    return problems


def _relation_matches(request_id, case, lookup, edition, optimization,
                      overflow_checks):
    key_base = (
        f"{request_id}/{case['id']}/{edition}/{optimization}"
        f"/{int(overflow_checks)}"
    )
    row_a = lookup.get(f"{key_base}/a")
    row_b = lookup.get(f"{key_base}/b")
    complete = (
        row_a is not None and row_b is not None
        and not row_a["compile"]["timed_out"] and not row_b["compile"]["timed_out"]
        and row_a["compile"]["exit"] is not None
        and row_b["compile"]["exit"] is not None
        and (
            (row_a["run"] is None or not row_a["run"]["timed_out"])
            and (row_b["run"] is None or not row_b["run"]["timed_out"])
            and (row_a["run"] is None or row_a["run"]["exit"] is not None)
            and (row_b["run"] is None or row_b["run"]["exit"] is not None)
        )
    )
    compiled = complete and row_a["compile"]["exit"] == row_b["compile"]["exit"] == 0
    equal = complete and _observation(row_a) == _observation(row_b)
    if case["expected"] == "same":
        matches = compiled and equal
    elif case["expected"] == "different":
        matches = complete and not equal
    else:
        matches = complete and (equal == (not overflow_checks)) \
            and (overflow_checks or compiled)
    return matches, complete, equal


def _build_proc_macro(scratch_root, request_id, toolchain, deadline):
    macro_path = scratch_root / "libobs.so"
    return run_process(
        [
            "rustc",
            f"+{toolchain}",
            "--edition",
            "2021",
            "--crate-type",
            "proc-macro",
            "--crate-name",
            "obs",
            "--extern",
            "proc_macro",
            str(MACRO_SOURCES["obs"]),
            "-o",
            str(macro_path),
            "-C",
            "opt-level=2",
        ],
        request_id=f"{request_id}/obs-macro",
        deadline=deadline,
        timeout=COMPILE_CAP,
    )


def run_matrix(cases, *, request_id, toolchain="stable", timeout=DEFAULT_TIMEOUT):
    """Verify every case side under every configuration within one deadline."""
    started = time.monotonic()
    if not math.isfinite(timeout) or timeout <= 0:
        raise ValueError("timeout must be finite and positive")
    deadline = started + float(timeout)
    work_deadline = deadline - GRACE_SECONDS
    validated = _validate_cases(cases)

    compiler_record = run_process(
        ["rustc", f"+{toolchain}", "--version"],
        request_id=f"{request_id}/compiler",
        deadline=deadline,
        timeout=PROBE_CAP,
    )

    needs_macro = any(case["externs"] for case in validated)
    macro_record = None
    scratch_root = pathlib.Path(
        tempfile.mkdtemp(prefix="dejadoc-constants-witness-"))
    try:
        if needs_macro and compiler_record["exit"] == 0:
            macro_record = _build_proc_macro(scratch_root, request_id,
                                             toolchain, deadline)
        macro_failed = needs_macro and (
            macro_record is None or macro_record["exit"] != 0)

        jobs = []
        for case in validated:
            for edition in EDITIONS:
                for optimization in OPTIMIZATIONS:
                    for overflow_checks in OVERFLOW_CHECKS:
                        for side in SIDES:
                            jobs.append(
                                (case, edition, optimization, overflow_checks,
                                 side))

        def row_id_for(job):
            case, edition, optimization, overflow_checks, side = job
            return (
                f"{request_id}/{case['id']}/{edition}/{optimization}"
                f"/{int(overflow_checks)}/{side}"
            )

        expected_ids = {row_id_for(job) for job in jobs}

        def failed_result(job, reason):
            case, edition, optimization, overflow_checks, side = job
            source = case["sources"][side]
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
                    "norm_stderr": reason,
                    "timed_out": time.monotonic() >= work_deadline,
                },
                "run": None,
            }

        results = []
        executor = concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS)
        pending = {}
        remaining_jobs = iter(jobs)

        def submit_next():
            if time.monotonic() >= work_deadline or compiler_record["exit"] != 0:
                return
            job = next(remaining_jobs, None)
            if job is not None:
                if macro_failed and job[0]["externs"]:
                    results.append(failed_result(job, "proc macro build "
                                                          "failed"))
                else:
                    future = executor.submit(
                        _run_job, scratch_root, request_id, toolchain,
                        deadline, job[0], job[0]["externs"], job[1], job[2],
                        job[3], job[4])
                    pending[future] = job

        def collect(future, job, bound):
            try:
                row = future.result(timeout=bound)
                if row["request_id"] != row_id_for(job):
                    row = failed_result(job, "worker returned an unrelated "
                                              "request id")
            except Exception as error:
                row = failed_result(job, f"worker error {error}")
            results.append(row)

        try:
            for _ in range(WORKERS):
                submit_next()
            while pending:
                completed, _ = concurrent.futures.wait(
                    pending,
                    timeout=max(0.0, work_deadline - time.monotonic()),
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
            reason = ("proc macro build failed"
                      if macro_failed and job[0]["externs"]
                      else "job not started before the deadline")
            results.append(failed_result(job, reason))
    finally:
        import shutil

        shutil.rmtree(scratch_root, ignore_errors=True)

    lookup = {}
    duplicates = []
    for row in results:
        if row["request_id"] in lookup:
            duplicates.append(row["request_id"])
        lookup[row["request_id"]] = row
    missing = sorted(expected_ids - set(lookup))

    checks = []
    for case in validated:
        for edition in EDITIONS:
            for optimization in OPTIMIZATIONS:
                for overflow_checks in OVERFLOW_CHECKS:
                    key = int(overflow_checks)
                    relation_matches, complete, equal = _relation_matches(
                        request_id, case, lookup, edition, optimization,
                        overflow_checks)
                    side_problems = []
                    for side in SIDES:
                        expectation = case["expect"][side][
                            "off" if key == 0 else "on"]
                        row = lookup.get(
                            f"{request_id}/{case['id']}/{edition}/{optimization}"
                            f"/{key}/{side}")
                        side_problems.extend(
                            _side_matches(row, expectation, case["id"], side,
                                          "off" if key == 0 else "on"))
                    checks.append({
                        "case_id": case["id"],
                        "family": case["family"],
                        "edition": edition,
                        "optimization": optimization,
                        "overflow_checks": overflow_checks,
                        "relation_matches": relation_matches,
                        "complete": complete,
                        "equal": equal,
                        "side_problems": side_problems,
                        "a_request_id":
                            f"{request_id}/{case['id']}/{edition}/{optimization}"
                            f"/{key}/a",
                        "b_request_id":
                            f"{request_id}/{case['id']}/{edition}/{optimization}"
                            f"/{key}/b",
                    })
    problem = None
    if missing:
        problem = f"missing results {len(missing)} of {len(expected_ids)} for {request_id}"
    elif duplicates:
        problem = f"duplicate results {len(duplicates)} for {request_id}"
    if problem is None and macro_failed:
        problem = f"proc macro build failed for {request_id}"
    satisfied = all(
        check["relation_matches"] and not check["side_problems"]
        for check in checks)
    ok = (
        problem is None and compiler_record["exit"] == 0 and satisfied
    )
    order = {row_id: index for index, row_id in enumerate(sorted(expected_ids))}
    results.sort(key=lambda row: order.get(row["request_id"], len(order)))
    return {
        "request_id": request_id,
        "ok": ok,
        "compiler": compiler_record,
        "proc_macro": macro_record,
        "results": results,
        "checks": checks,
        "error": problem,
        "elapsed_seconds": round(time.monotonic() - started, 3),
    }


def main(argv=None):
    parser = argparse.ArgumentParser(
        prog="dependency_constants_witnesses",
        description="Verify the M2 checked-constant native witnesses.",
    )
    parser.add_argument(
        "--cases",
        default=str(DEFAULT_CASES),
        help="fixture path holding the studied source pairs",
    )
    parser.add_argument(
        "--output",
        help="report path. omit to print the report on standard output",
    )
    parser.add_argument("--request-id", default=f"M2C-WITNESS-{int(time.time())}")
    parser.add_argument("--toolchain", default="stable")
    parser.add_argument(
        "--timeout",
        type=float,
        default=DEFAULT_TIMEOUT,
        help="global wall clock budget in seconds",
    )
    parser.add_argument(
        "--case",
        action="append",
        default=None,
        help="case id filter. repeatable",
    )
    args = parser.parse_args(argv)
    if args.timeout <= 0:
        print("dependency_constants_witnesses timeout must be positive",
              file=sys.stderr)
        return 2
    try:
        cases = json.loads(pathlib.Path(args.cases).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        print(f"dependency_constants_witnesses cannot read {args.cases} {error}",
              file=sys.stderr)
        return 2
    if not isinstance(cases, list):
        print("dependency_constants_witnesses cases must be a JSON list",
              file=sys.stderr)
        return 2
    if args.case:
        known = {case.get("id") for case in cases if isinstance(case, dict)}
        unknown = [value for value in args.case if value not in known]
        if unknown:
            print(
                f"dependency_constants_witnesses unknown case ids "
                f"{', '.join(unknown)}",
                file=sys.stderr,
            )
            return 2
        wanted = set(args.case)
        cases = [case for case in cases
                 if isinstance(case, dict) and case.get("id") in wanted]
    try:
        report = run_matrix(
            cases,
            request_id=args.request_id,
            toolchain=args.toolchain,
            timeout=args.timeout,
        )
    except ValueError as error:
        print(f"dependency_constants_witnesses invalid fixture {error}",
              file=sys.stderr)
        return 2
    text = json.dumps(report, indent=2)
    if args.output:
        try:
            pathlib.Path(args.output).write_text(text + "\n", encoding="utf-8")
        except OSError as error:
            print(
                f"dependency_constants_witnesses cannot write {args.output} "
                f"{error}",
                file=sys.stderr,
            )
            return 2
    else:
        print(text)
    if report["error"]:
        print(report["error"], file=sys.stderr)
    mismatched = sorted(
        {check["case_id"] for check in report["checks"]
         if not check["relation_matches"] or check["side_problems"]})
    families = {}
    for check in report["checks"]:
        families.setdefault(check["family"], 0)
        families[check["family"]] += 1
    family_summary = ", ".join(
        f"{name}:{count}" for name, count in sorted(families.items()))
    print(
        f"{report['request_id']} ok={int(report['ok'])} "
        f"results={len(report['results'])} checks={len(report['checks'])} "
        f"families={family_summary or '-'} "
        f"mismatches={','.join(mismatched) or '-'}"
    )
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
