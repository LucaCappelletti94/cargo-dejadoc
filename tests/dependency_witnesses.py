"""Bounded compiler and runtime witness driver for dependency canonicalization fixtures."""

import argparse
import concurrent.futures
import hashlib
import json
import math
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time

EDITIONS = ("2021", "2024")
OPTIMIZATIONS = (0, 2)
OVERFLOW_CHECKS = (False, True)
SIDES = ("a", "b")
EXPECTATIONS = frozenset(("same", "different", "profile-dependent"))
COMPILE_CAP = 30.0
RUN_CAP = 5.0
PROBE_CAP = 10.0
GRACE_SECONDS = 2.0
WORKERS = 2
NICE = ["nice", "-n", "10"] if os.name == "posix" and shutil.which("nice") else []
DEFAULT_TIMEOUT = 600.0
DEFAULT_CASES = (
    pathlib.Path(__file__).resolve().with_name("fixtures").joinpath("dependency", "cases.json")
)


def _with_nice(command):
    return [*NICE, *command]


def _kill_group(child):
    if os.name == "posix":
        try:
            os.killpg(child.pid, signal.SIGKILL)
            return
        except ProcessLookupError:
            return
    child.kill()


def run_process(command, *, request_id, deadline, timeout):
    """Run one subprocess under monotonic bounds and return its correlated record."""
    record = {
        "request_id": request_id,
        "exit": None,
        "stdout": "",
        "stderr": "",
        "timed_out": False,
    }
    started = time.monotonic()
    operation_end = min(started + timeout, deadline - GRACE_SECONDS)
    cleanup_end = min(operation_end + GRACE_SECONDS, deadline)
    if operation_end <= started:
        record["timed_out"] = True
        record["stderr"] = f"{request_id} global deadline exceeded"
        return record
    try:
        child = subprocess.Popen(
            _with_nice(command),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
    except OSError as error:
        record["stderr"] = f"{request_id} spawn failed {error}"
        return record
    stdout = b""
    stderr = b""
    try:
        try:
            stdout, stderr = child.communicate(
                timeout=max(0.0, operation_end - time.monotonic())
            )
            record["exit"] = child.returncode
        except subprocess.TimeoutExpired:
            record["timed_out"] = True
            _kill_group(child)
            try:
                stdout, stderr = child.communicate(
                    timeout=max(0.0, cleanup_end - time.monotonic())
                )
            except subprocess.TimeoutExpired as error:
                stdout, stderr = error.output or b"", error.stderr or b""
                record["error"] = f"{request_id} cleanup deadline exceeded"
    finally:
        child.stdout.close()
        child.stderr.close()
        if child.returncode is None:
            _kill_group(child)
            try:
                child.wait(timeout=max(0.0, cleanup_end - time.monotonic()))
            except subprocess.TimeoutExpired:
                record["error"] = f"{request_id} child was not reaped"
    record["stdout"] = stdout.decode("utf-8", "replace")
    record["stderr"] = stderr.decode("utf-8", "replace")
    return record


def _validate_cases(cases):
    if not isinstance(cases, (list, tuple)) or not cases:
        raise ValueError("cases must be a nonempty list of objects")
    validated = []
    seen = set()
    for position, case in enumerate(cases):
        if not isinstance(case, dict):
            raise ValueError(f"case at position {position} is not an object")
        case_id = case.get("id")
        source_a = case.get("a")
        source_b = case.get("b")
        expected = case.get("expected")
        if not isinstance(case_id, str) or not case_id:
            raise ValueError(f"case at position {position} has no id")
        if any(part in case_id for part in ("/", "\\", "\x00", "..")):
            raise ValueError(f"case {case_id} has an unusable id")
        if case_id in seen:
            raise ValueError(f"duplicate case id {case_id}")
        seen.add(case_id)
        if not isinstance(source_a, str) or not source_a:
            raise ValueError(f"case {case_id} is missing source a")
        if not isinstance(source_b, str) or not source_b:
            raise ValueError(f"case {case_id} is missing source b")
        if not isinstance(expected, str) or expected not in EXPECTATIONS:
            raise ValueError(f"case {case_id} has unsupported expectation {expected!r}")
        validated.append((case_id, source_a, source_b, expected))
    return validated


def _run_job(scratch_root, request_id, toolchain, deadline, sources, job):
    case_id, edition, optimization, overflow_checks, side = job
    row_id = f"{request_id}/{case_id}/{edition}/{optimization}/{int(overflow_checks)}/{side}"
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
    compile_record = run_process(
        command,
        request_id=row_id + "/compile",
        deadline=deadline,
        timeout=COMPILE_CAP,
    )
    if compile_record["exit"] != 0:
        run_record = None
    else:
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


def _build_checks(validated, lookup, request_id):
    checks = []
    for case_id, source_a, source_b, expected in validated:
        a_sha = hashlib.sha256(source_a.encode("utf-8")).hexdigest()
        b_sha = hashlib.sha256(source_b.encode("utf-8")).hexdigest()
        for edition in EDITIONS:
            for optimization in OPTIMIZATIONS:
                for overflow_checks in OVERFLOW_CHECKS:
                    prefix = (
                        f"{request_id}/{case_id}/{edition}/{optimization}"
                        f"/{int(overflow_checks)}"
                    )
                    row_a = lookup.get(prefix + "/a")
                    row_b = lookup.get(prefix + "/b")
                    run_a = row_a.get("run") if row_a else None
                    run_b = row_b.get("run") if row_b else None
                    compiled = (
                        row_a is not None
                        and row_b is not None
                        and row_a["compile"]["exit"] == 0
                        and row_b["compile"]["exit"] == 0
                        and not row_a["compile"]["timed_out"]
                        and not row_b["compile"]["timed_out"]
                    )
                    complete = (
                        compiled
                        and run_a is not None
                        and run_b is not None
                        and not run_a["timed_out"]
                        and not run_b["timed_out"]
                        and run_a["exit"] is not None
                        and run_b["exit"] is not None
                    )
                    equal = complete and (run_a["exit"], run_a["stdout"]) == (
                        run_b["exit"],
                        run_b["stdout"],
                    )
                    if expected == "same":
                        matches = complete and equal
                    elif expected == "different":
                        matches = complete and not equal
                    else:
                        matches = complete and equal == (not overflow_checks)
                    checks.append(
                        {
                            "case_id": case_id,
                            "edition": edition,
                            "optimization": optimization,
                            "overflow_checks": overflow_checks,
                            "compiled": compiled,
                            "equal": equal,
                            "matches_expectation": matches,
                            "a_request_id": prefix + "/a",
                            "b_request_id": prefix + "/b",
                            "a_sha256": a_sha,
                            "b_sha256": b_sha,
                        }
                    )
    return checks


def run_matrix(cases, *, request_id, toolchain="stable", timeout=DEFAULT_TIMEOUT):
    """Run every edition, optimization, overflow and side combination under one deadline."""
    started = time.monotonic()
    timeout = float(timeout)
    if not math.isfinite(timeout) or timeout <= 0:
        raise ValueError("timeout must be finite and positive")
    deadline = started + float(timeout)
    work_deadline = deadline - GRACE_SECONDS
    validated = _validate_cases(cases)
    sources = {
        case_id: (source_a, source_b) for case_id, source_a, source_b, _ in validated
    }
    compiler_record = run_process(
        ["rustc", f"+{toolchain}", "--version"],
        request_id=f"{request_id}/compiler",
        deadline=deadline,
        timeout=PROBE_CAP,
    )

    def row_id_for(job):
        case_id, edition, optimization, overflow_checks, side = job
        return f"{request_id}/{case_id}/{edition}/{optimization}/{int(overflow_checks)}/{side}"

    jobs = []
    for case_id, _, _, _ in validated:
        for edition in EDITIONS:
            for optimization in OPTIMIZATIONS:
                for overflow_checks in OVERFLOW_CHECKS:
                    for side in SIDES:
                        jobs.append((case_id, edition, optimization, overflow_checks, side))
    expected_ids = {row_id_for(job) for job in jobs}

    def failed_result(job, reason):
        case_id, edition, optimization, overflow_checks, side = job
        source = sources[case_id][0] if side == "a" else sources[case_id][1]
        row_id = row_id_for(job)
        return {
            "request_id": row_id,
            "side": side,
            "edition": edition,
            "optimization": optimization,
            "overflow_checks": overflow_checks,
            "sha256": hashlib.sha256(source.encode("utf-8")).hexdigest(),
            "compile": {
                "request_id": row_id + "/compile",
                "exit": None,
                "stdout": "",
                "stderr": reason,
                "timed_out": True,
            },
            "run": None,
        }

    results = []
    with tempfile.TemporaryDirectory(prefix="dejadoc-witness-") as scratch:
        scratch_root = pathlib.Path(scratch)
        executor = concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS)
        pending = {}
        remaining_jobs = iter(jobs)

        def submit_next():
            if time.monotonic() >= work_deadline or compiler_record["exit"] != 0:
                return
            job = next(remaining_jobs, None)
            if job is not None:
                future = executor.submit(
                    _run_job, scratch_root, request_id, toolchain, deadline, sources, job
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
            results.append(failed_result(job, "job not started before the deadline"))
    lookup = {}
    duplicates = []
    for row in results:
        if row["request_id"] in lookup:
            duplicates.append(row["request_id"])
        lookup[row["request_id"]] = row
    missing = sorted(expected_ids - set(lookup))
    checks = _build_checks(validated, lookup, request_id)
    problem = None
    if missing:
        problem = f"missing results {len(missing)} of {len(expected_ids)} for {request_id}"
    elif duplicates:
        problem = f"duplicate results {len(duplicates)} for {request_id}"
    satisfied = all(check["matches_expectation"] for check in checks)
    ok = problem is None and compiler_record["exit"] == 0 and satisfied
    order = {row_id: index for index, row_id in enumerate(sorted(expected_ids))}
    results.sort(key=lambda row: order.get(row["request_id"], len(order)))
    return {
        "request_id": request_id,
        "ok": ok,
        "compiler": compiler_record,
        "results": results,
        "checks": checks,
        "error": problem,
        "elapsed_seconds": round(time.monotonic() - started, 3),
    }


def main(argv=None):
    parser = argparse.ArgumentParser(
        prog="dependency_witnesses",
        description="Run the bounded compiler and runtime witness matrix.",
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
    parser.add_argument("--request-id", default=f"DG-WITNESS-{int(time.time())}")
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
        print("dependency_witnesses timeout must be positive", file=sys.stderr)
        return 2
    try:
        cases = json.loads(pathlib.Path(args.cases).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        print(f"dependency_witnesses cannot read {args.cases} {error}", file=sys.stderr)
        return 2
    if not isinstance(cases, list):
        print("dependency_witnesses cases must be a JSON list", file=sys.stderr)
        return 2
    if args.case:
        known = {case.get("id") for case in cases if isinstance(case, dict)}
        unknown = [value for value in args.case if value not in known]
        if unknown:
            print(
                f"dependency_witnesses unknown case ids {', '.join(unknown)}",
                file=sys.stderr,
            )
            return 2
        wanted = set(args.case)
        cases = [case for case in cases if isinstance(case, dict) and case.get("id") in wanted]
    try:
        report = run_matrix(
            cases,
            request_id=args.request_id,
            toolchain=args.toolchain,
            timeout=args.timeout,
        )
    except ValueError as error:
        print(f"dependency_witnesses invalid fixture {error}", file=sys.stderr)
        return 2
    text = json.dumps(report, indent=2)
    if args.output:
        try:
            pathlib.Path(args.output).write_text(text + "\n", encoding="utf-8")
        except OSError as error:
            print(f"dependency_witnesses cannot write {args.output} {error}", file=sys.stderr)
            return 2
    else:
        print(text)
    if report["error"]:
        print(report["error"], file=sys.stderr)
    mismatches = sorted(
        {check["case_id"] for check in report["checks"] if not check["matches_expectation"]}
    )
    print(
        f"{report['request_id']} ok={int(report['ok'])} "
        f"results={len(report['results'])} checks={len(report['checks'])} "
        f"mismatches={','.join(mismatches) or '-'}"
    )
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
