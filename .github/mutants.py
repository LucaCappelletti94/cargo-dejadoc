"""Run and audit bounded mutation partitions of the workspace or of one change."""

import argparse
import contextlib
import json
import math
import os
import pathlib
import signal
import subprocess
import sys
import time

VERSION = "27.1.0"
# Below this many mutants a shard spends most of its run on setup and the baselines.
PARTITION_FLOOR = 16
# The most mutants a shard tests within RUN_SECONDS at about a minute each.
PARTITION_LIMIT = 160
# Two waves of the 20 jobs the account runs at once.
MATRIX_LIMIT = 40
RUN_SECONDS = 240 * 60
CLEANUP_SECONDS = 5
CARGO = [
    "taskset", "-c", "0,1", "nice", "-n", "10", "env",
    "RUSTC_WRAPPER=sccache", "CARGO_BUILD_JOBS=2",
    "CARGO_PROFILE_TEST_DEBUG=0", "NEXTEST_PROFILE=mutants", "cargo",
]
BASELINE = [*CARGO, "nextest", "run", "--workspace", "--cargo-profile", "test"]
EXCLUSIONS = (
    r'delete match arm (syn::)?(ImplItem|TraitItem|ForeignItem)::Macro\(m\) in (impl_member|trait_member|foreign_member)',
    r'replace match guard mac\.path\.is_ident\("concat"\) with true',
    r'replace \|\| with && in single_segment_type_name',
    r'replace \+ with [-*] in merge_parts',
    r'replace > with >= in block_star_strip',
    r'delete ! in block_star_strip',
    r'replace == with != in block_star_strip',
    r'replace \+ with - in flat_literal',
    r'replace match guard rustdoc with (true|false) in mod_decls',
    r'replace match guard path.exists\(\) with true in mod_decls',
    r'^syn-canon/src/const_items\.rs:\d+:\d+: replace match guard (!self\.hazard with true|mac\.ident\.is_some\(\) with false) in .*Analysis>::visit_item$',
    r'^syn-canon/src/drift\.rs:\d+:\d+: delete match arm syn::Type::(Macro|Never)\(ty\) in type_attrs$',
    r'^syn-canon/src/drift\.rs:\d+:\d+: delete match arm syn::Expr::(Continue|Lit|Macro)\(expr\) in expr_attrs$',
    r'^syn-canon/src/drift\.rs:\d+:\d+: delete match arm syn::ForeignItem::Macro\(v\) in <impl VisitMut for Drift>::visit_foreign_item_mut$',
    r'^syn-canon/src/form\.rs:\d+:\d+: replace == with != in framed_tree$',
)
SELECTOR = [
    "--workspace", "--cap-lints", "true", "--timeout", "180",
    "--test-workspace", "true", "--jobs", "2", "--jobserver-tasks", "2",
    "--test-tool", "nextest", "--profile", "test", "--no-shuffle",
    *(arg for expression in EXCLUSIONS for arg in ("-E", expression)),
]


def require(condition, message):
    if not condition:
        raise ValueError(message)


def load(path):
    return json.loads(pathlib.Path(path).read_text(encoding="utf-8"))


def save(path, value):
    pathlib.Path(path).write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def names(mutants):
    require(isinstance(mutants, list), "Invalid mutant inventory")
    result = [mutant["name"] for mutant in mutants]
    require(all(isinstance(name, str) and name for name in result), "Invalid mutant identity")
    require(len(set(result)) == len(result), "Duplicate mutant identity")
    return result


def partitions(mutants, changes=None):
    """The contiguous shards `cargo mutants --sharding slice` runs, at most `MATRIX_LIMIT`."""
    names(mutants)
    require(mutants or changes, "Empty mutant inventory")
    if not mutants:
        return []
    size = max(PARTITION_FLOOR, math.ceil(len(mutants) / MATRIX_LIMIT))
    count = math.ceil(len(mutants) / size)
    chunk = math.ceil(len(mutants) / count)
    require(chunk <= PARTITION_LIMIT, "Mutation inventory exceeds the complete matrix capacity")
    return [mutants[index * chunk:(index + 1) * chunk] for index in range(count)]


def selector(changes):
    return [*SELECTOR, *(["--in-diff", changes] if changes else [])]


def list_command(changes):
    return [*CARGO, "mutants", *selector(changes), "--list", "--json"]


def mutation_command(changes, index, count):
    return [*CARGO, "mutants", *selector(changes), "--shard", f"{index}/{count}",
            "--sharding", "slice"]


def run(command, *, request_id, deadline, output, timeout=None, stdout=None):
    started = time.monotonic()
    operation_end = deadline - CLEANUP_SECONDS
    if timeout is not None:
        operation_end = min(operation_end, started + timeout)
    cleanup_end = min(deadline, operation_end + CLEANUP_SECONDS)
    record = {"request_id": request_id, "argv": command, "exit": None,
              "timed_out": False, "elapsed_seconds": 0.0}
    print(f"{request_id} deadline_seconds={max(0, operation_end - started):.1f}", flush=True)
    if operation_end <= started:
        record["timed_out"] = True
        return record
    with contextlib.ExitStack() as stack:
        log = stack.enter_context(pathlib.Path(output).open("w", encoding="utf-8"))
        out = stack.enter_context(pathlib.Path(stdout).open("w", encoding="utf-8")) if stdout else log
        child = subprocess.Popen(command, stdout=out, stderr=log, start_new_session=True)
        try:
            record["exit"] = child.wait(timeout=max(0, operation_end - time.monotonic()))
        except subprocess.TimeoutExpired:
            record["timed_out"] = True
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            try:
                record["exit"] = child.wait(timeout=max(0, cleanup_end - time.monotonic()))
            except subprocess.TimeoutExpired:
                record["error"] = "Process cleanup deadline exceeded"
    record["elapsed_seconds"] = time.monotonic() - started
    print(f"{request_id} exit={record['exit']} timed_out={record['timed_out']}", flush=True)
    return record


def successful(record, request_id, command):
    require(record["request_id"] == request_id, "Unrelated subprocess receipt")
    require(record["argv"] == command, "Unexpected subprocess command")
    require(record["exit"] == 0 and not record["timed_out"], "Subprocess failed or timed out")
    require("error" not in record, "Subprocess cleanup failed")


def write_changes(request_id, deadline, output, base, revision, path):
    """Write the diff from `base` to `revision` that scopes the inventory to one change."""
    command = ["git", "diff", "--no-color", "--no-ext-diff", base, revision]
    record = run(command, request_id=f"{request_id}/diff", deadline=deadline,
                 timeout=30, output=output / "diff.log", stdout=path)
    save(output / "diff-receipt.json", record)
    successful(record, f"{request_id}/diff", command)


def enumerate_mutants(request_id, deadline, output, changes):
    command = list_command(changes)
    record = run(command, request_id=f"{request_id}/list", deadline=deadline,
                 timeout=120, output=output / "list.log", stdout=output / "list.json")
    save(output / "list-receipt.json", record)
    successful(record, f"{request_id}/list", command)
    listing = (output / "list.json").read_text(encoding="utf-8")
    # A change without Rust sources leaves the listing empty.
    mutants = json.loads(listing) if listing or not changes else []
    names(mutants)
    return mutants


def inventory(request_id, revision, mutants, changes=None):
    slices = partitions(mutants, changes)
    matrix = {"include": [{"index": index, "count": len(slices), "label": index + 1}
                          for index in range(len(slices))]}
    return {"request_id": request_id, "revision": revision, "version": VERSION,
            "selector": selector(changes), "changes": changes, "mutants": mutants,
            "matrix": matrix}


def validate_inventory(value, request_id, revision):
    expected = inventory(request_id, revision, value["mutants"], value["changes"])
    require(value == expected, "Unexpected mutation inventory or execution selector")
    return partitions(value["mutants"], value["changes"])


def validate_outcomes(folder, expected):
    require(load(folder / "mutants.json") == expected, "Shard mutant enumeration differs")
    result = load(folder / "outcomes.json")
    require(result["cargo_mutants_version"] == VERSION, "Unexpected mutation tool version")
    require(result["end_time"] is not None, "Mutation execution is incomplete")
    outcomes = result["outcomes"]
    baselines = [row for row in outcomes if row["scenario"] == "Baseline"]
    require(len(baselines) == 1, "Missing or duplicate mutation baseline")
    require(baselines[0]["summary"] == "Success", "Mutation baseline failed")
    require([(phase["phase"], phase["process_status"])
             for phase in baselines[0]["phase_results"]] ==
            [("Build", "Success"), ("Test", "Success")], "Mutation baseline phases differ")
    observed = {}
    counts = {"CaughtMutant": 0, "Unviable": 0}
    for row in outcomes:
        if row["scenario"] == "Baseline":
            continue
        mutant = row["scenario"]["Mutant"]
        name = mutant["name"]
        require(name not in observed, "Duplicate mutation outcome")
        observed[name] = mutant
        summary = row["summary"]
        require(summary in counts, "Mutation was missed, timed out or failed")
        phases = row["phase_results"]
        require([phase["phase"] for phase in phases] ==
                (["Build", "Test"] if summary == "CaughtMutant" else ["Build"]),
                "Mutation terminal phases differ")
        for phase in phases:
            require("--workspace" in phase["argv"] and "--cargo-profile=test" in phase["argv"],
                    "Mutation test scope or profile differs")
        if summary == "CaughtMutant":
            require(phases[0]["process_status"] == "Success", "Caught mutation build failed")
        terminal = phases[-1]["process_status"]
        require(isinstance(terminal, dict) and set(terminal) == {"Failure"},
                "Mutation has no terminal failure")
        counts[summary] += 1
    identities = {mutant["name"]: {key: value for key, value in mutant.items() if key != "diff"}
                  for mutant in expected}
    require(observed == identities, "Mutation outcomes do not cover the exact shard")
    expected_counts = {"total_mutants": len(expected), "caught": counts["CaughtMutant"],
                       "unviable": counts["Unviable"], "missed": 0, "timeout": 0, "success": 0}
    require(all(result[key] == value for key, value in expected_counts.items()),
            "Mutation outcome counters differ")
    return expected_counts


def run_shard(args, value, deadline):
    slices = validate_inventory(value, args.request_id, args.revision)
    require(args.index is not None and 0 <= args.index < len(slices), "Unexpected shard index")
    expected = slices[args.index]
    shard_id = f"{args.request_id}/shard/{args.index}"
    receipt = {"request_id": shard_id, "index": args.index, "count": len(slices),
               "revision": args.revision, "ok": False}
    output = args.output
    try:
        mutants = enumerate_mutants(shard_id, deadline, output, value["changes"])
        require(mutants == value["mutants"], "Workspace enumeration differs from inventory")
        receipt["baseline"] = run(BASELINE, request_id=f"{shard_id}/baseline",
                                  deadline=deadline, output=output / "workspace-baseline.log")
        save(output / "receipt.json", receipt)
        successful(receipt["baseline"], f"{shard_id}/baseline", BASELINE)
        command = mutation_command(value["changes"], args.index, len(slices))
        receipt["mutation"] = run(command, request_id=f"{shard_id}/mutants",
                                  deadline=deadline, output=output / "mutation.log")
        save(output / "receipt.json", receipt)
        successful(receipt["mutation"], f"{shard_id}/mutants", command)
        receipt["counts"] = validate_outcomes(pathlib.Path("mutants.out"), expected)
        require(time.monotonic() < deadline, "Shard validation deadline exceeded")
        receipt["ok"] = True
    except (OSError, ValueError, KeyError, TypeError) as error:
        receipt["error"] = str(error)
        raise
    finally:
        save(output / "receipt.json", receipt)


def aggregate(args, value, deadline):
    slices = validate_inventory(value, args.request_id, args.revision)
    folders = sorted(args.artifacts.glob("mutants-shard-*"))
    require(len(folders) == len(slices), "Missing or unexpected shard artifacts")
    observed = set()
    union = []
    for folder in folders:
        require(time.monotonic() < deadline, "Aggregation deadline exceeded")
        receipt = load(folder / "mutation-results" / "receipt.json")
        index = receipt["index"]
        require(isinstance(index, int) and 0 <= index < len(slices) and index not in observed,
                "Unexpected or duplicate shard index")
        observed.add(index)
        shard_id = f"{args.request_id}/shard/{index}"
        require(receipt["request_id"] == shard_id and receipt["count"] == len(slices) and
                receipt["revision"] == args.revision and receipt["ok"] is True,
                "Incomplete or unrelated shard receipt")
        successful(load(folder / "mutation-results" / "list-receipt.json"),
                   f"{shard_id}/list", list_command(value["changes"]))
        require(load(folder / "mutation-results" / "list.json") == value["mutants"],
                "Shard workspace inventory differs")
        successful(receipt["baseline"], f"{shard_id}/baseline", BASELINE)
        successful(receipt["mutation"], f"{shard_id}/mutants",
                   mutation_command(value["changes"], index, len(slices)))
        counts = validate_outcomes(folder / "mutants.out", slices[index])
        require(receipt["counts"] == counts, "Shard receipt counts differ")
        union.extend(names(slices[index]))
    require(len(union) == len(set(union)) and set(union) == set(names(value["mutants"])),
            "Shard membership is incomplete or overlapping")
    require(time.monotonic() < deadline, "Aggregation deadline exceeded")
    save(args.output / "aggregate.json", {"request_id": args.request_id,
         "revision": args.revision, "ok": True, "shards": len(slices), "mutants": len(union)})
    print(f"{args.request_id} complete_mutants={len(union)} shards={len(slices)}", flush=True)


def main():
    """Execute one correlated enumeration, shard or aggregation job."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("enumerate", "shard", "aggregate"))
    parser.add_argument("--request-id", required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--inventory", type=pathlib.Path, default=pathlib.Path("mutation-inventory/inventory.json"))
    parser.add_argument("--output", type=pathlib.Path, default=pathlib.Path("mutation-results"))
    parser.add_argument("--artifacts", type=pathlib.Path, default=pathlib.Path("mutation-artifacts"))
    parser.add_argument("--index", type=int)
    parser.add_argument("--base", help="scope the inventory to the change from this commit")
    args = parser.parse_args()
    started = time.monotonic()
    deadline = started + (RUN_SECONDS if args.mode == "shard" else 120)
    args.output.mkdir(parents=True, exist_ok=True)
    try:
        if args.mode == "enumerate":
            args.inventory.parent.mkdir(parents=True, exist_ok=True)
            changes = None
            if args.base:
                changes = str(args.inventory.parent / "changes.diff")
                write_changes(args.request_id, deadline, args.output, args.base, args.revision,
                              changes)
            mutants = enumerate_mutants(args.request_id, deadline, args.output, changes)
            value = inventory(args.request_id, args.revision, mutants, changes)
            require(time.monotonic() < deadline, "Enumeration deadline exceeded")
            save(args.inventory, value)
            with pathlib.Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as output:
                output.write("matrix=" + json.dumps(value["matrix"], separators=(",", ":")) + "\n")
                output.write(f"shards={len(value['matrix']['include'])}\n")
        elif args.mode == "shard":
            run_shard(args, load(args.inventory), deadline)
        else:
            aggregate(args, load(args.inventory), deadline)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"{args.request_id} failed {error}", file=sys.stderr, flush=True)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
