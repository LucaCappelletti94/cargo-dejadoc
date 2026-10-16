"""CLI contract tests for the bounded mutation partition runner."""

import importlib.util
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

from dependency_witnesses import run_process

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
RUNNER = REPO_ROOT / ".github" / "mutants.py"
REQUEST_ID = "MUTANTS-TEST-1-1"
REVISION = "test-revision"
BUILD_ARGV = ["cargo", "nextest", "run", "--no-run", "--cargo-profile=test", "--verbose", "--workspace"]
TEST_ARGV = ["cargo", "nextest", "run", "--cargo-profile=test", "--verbose", "--workspace"]


def load_runner():
    spec = importlib.util.spec_from_file_location("mutants_runner", RUNNER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


RUNNER_MODULE = load_runner()


def make_mutants(count):
    mutants = []
    for index in range(count):
        line = 150 + index * 2
        mutants.append({
            "name": "syn-canon/src/helpers.rs:{0}:28: replace == with != in HelperTable::resolve_names".format(line),
            "package": "syn-canon",
            "file": "syn-canon/src/helpers.rs",
            "function": {
                "function_name": "HelperTable::resolve_names",
                "return_type": "-> Option<HelperId>",
                "span": {"start": {"line": 140, "column": 5}, "end": {"line": 157, "column": 6}},
            },
            "span": {"start": {"line": line, "column": 28}, "end": {"line": line, "column": 94}},
            "replacement": "true",
            "genre": "MatchArmGuard",
        })
    return mutants


def list_command():
    return [*RUNNER_MODULE.CARGO, "mutants", *RUNNER_MODULE.SELECTOR, "--list", "--json"]


def baseline_command():
    return [*RUNNER_MODULE.CARGO, "nextest", "run", "--workspace", "--cargo-profile", "test"]


def mutation_command(index, count):
    return [*RUNNER_MODULE.CARGO, "mutants", *RUNNER_MODULE.SELECTOR,
            "--shard", "{0}/{1}".format(index, count), "--sharding", "slice"]


def subprocess_receipt(request_id, argv, exit_code=0, timed_out=False):
    return {"request_id": request_id, "argv": argv, "exit": exit_code,
            "timed_out": timed_out, "elapsed_seconds": 1.5}


def phase(name, status):
    argv = BUILD_ARGV if name == "Build" else TEST_ARGV
    return {"phase": name, "duration": 1.5, "process_status": status, "argv": argv}


def make_outcomes(mutants):
    rows = [{"scenario": "Baseline", "summary": "Success",
             "phase_results": [phase("Build", "Success"), phase("Test", "Success")]}]
    caught = 0
    unviable = 0
    for index, mutant in enumerate(mutants):
        scenario = {"Mutant": {key: value for key, value in mutant.items() if key != "diff"}}
        if index == 0 and len(mutants) > 1:
            rows.append({"scenario": scenario, "summary": "Unviable",
                         "log_path": "log/mutant.log", "diff_path": "diff/mutant.diff",
                         "phase_results": [phase("Build", {"Failure": 101})]})
            unviable += 1
        else:
            rows.append({"scenario": scenario, "summary": "CaughtMutant",
                         "log_path": "log/mutant.log", "diff_path": "diff/mutant.diff",
                         "phase_results": [phase("Build", "Success"), phase("Test", {"Failure": 100})]})
            caught += 1
    return {"outcomes": rows, "total_mutants": len(mutants), "missed": 0,
            "caught": caught, "timeout": 0, "unviable": unviable, "success": 0,
            "start_time": "2026-10-12T13:22:49.279608104Z",
            "end_time": "2026-10-12T13:46:33.766515824Z",
            "cargo_mutants_version": RUNNER_MODULE.VERSION}


def make_shard(artifacts, index, count, mutants, slice_mutants):
    shard_id = "{0}/shard/{1}".format(REQUEST_ID, index)
    folder = artifacts / "mutants-shard-{0}-of-{1}".format(index + 1, count)
    results = folder / "mutation-results"
    results.mkdir(parents=True)
    (folder / "mutants.out").mkdir()
    outcomes = make_outcomes(slice_mutants)
    receipt = {
        "request_id": shard_id,
        "index": index,
        "count": count,
        "revision": REVISION,
        "ok": True,
        "baseline": subprocess_receipt(shard_id + "/baseline", baseline_command()),
        "mutation": subprocess_receipt(shard_id + "/mutants", mutation_command(index, count)),
        "counts": {key: outcomes[key]
                   for key in ("total_mutants", "caught", "unviable", "missed", "timeout", "success")},
    }
    (results / "list.json").write_text(json.dumps(mutants), encoding="utf-8")
    (results / "list-receipt.json").write_text(
        json.dumps(subprocess_receipt(shard_id + "/list", list_command())), encoding="utf-8")
    (results / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    (folder / "mutants.out" / "mutants.json").write_text(json.dumps(slice_mutants), encoding="utf-8")
    (folder / "mutants.out" / "outcomes.json").write_text(json.dumps(outcomes), encoding="utf-8")
    return folder


def build_fixture(directory, count=33):
    mutants = make_mutants(count)
    value = RUNNER_MODULE.inventory(REQUEST_ID, REVISION, mutants)
    artifacts = directory / "mutation-artifacts"
    inventory = artifacts / "mutation-inventory" / "inventory.json"
    inventory.parent.mkdir(parents=True)
    inventory.write_text(json.dumps(value), encoding="utf-8")
    slices = RUNNER_MODULE.partitions(mutants)
    for index, slice_mutants in enumerate(slices):
        make_shard(artifacts, index, len(slices), mutants, slice_mutants)
    return value


def run_cli(directory, *arguments):
    arguments = list(arguments)
    for option in ("--inventory", "--output", "--artifacts"):
        if option in arguments:
            index = arguments.index(option) + 1
            arguments[index] = str(pathlib.Path(directory) / arguments[index])
    command = [sys.executable, str(RUNNER), *arguments]
    result = run_process(
        command,
        request_id=f"{REQUEST_ID}/{pathlib.Path(directory).name}/{arguments[0]}",
        deadline=time.monotonic() + 120,
        timeout=120,
    )
    return subprocess.CompletedProcess(
        command, result["exit"], stdout=result["stdout"], stderr=result["stderr"]
    )


class ShardCliTests(unittest.TestCase):
    def run_shard(self, directory, inventory, index):
        return run_cli(directory, "shard", "--request-id", REQUEST_ID, "--revision", REVISION,
                       "--inventory", str(inventory), "--output", str(directory / "shard-output"),
                       "--artifacts", str(directory / "shard-artifacts"), "--index", str(index))

    def test_mismatched_inventory_fails_before_any_subprocess(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            mutants = make_mutants(4)
            value = RUNNER_MODULE.inventory(REQUEST_ID, REVISION, mutants)
            value["selector"] = ["--workspace"]
            inventory = directory / "inventory.json"
            inventory.write_text(json.dumps(value), encoding="utf-8")
            process = self.run_shard(directory, inventory, 0)
            self.assertEqual(process.returncode, 1, process.stderr)
            self.assertIn("Unexpected mutation inventory or execution selector", process.stderr)
            self.assertIn(REQUEST_ID, process.stderr)

    def test_out_of_range_index_fails_before_any_subprocess(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            mutants = make_mutants(4)
            value = RUNNER_MODULE.inventory(REQUEST_ID, REVISION, mutants)
            inventory = directory / "inventory.json"
            inventory.write_text(json.dumps(value), encoding="utf-8")
            process = self.run_shard(directory, inventory, 9)
            self.assertEqual(process.returncode, 1, process.stderr)
            self.assertIn("Unexpected shard index", process.stderr)


class AggregateContractTests(unittest.TestCase):
    def run_aggregate(self, directory):
        return run_cli(directory, "aggregate", "--request-id", REQUEST_ID, "--revision", REVISION,
                       "--inventory", "mutation-artifacts/mutation-inventory/inventory.json",
                       "--output", "aggregate-output", "--artifacts", "mutation-artifacts")

    def assert_rejected(self, process, message, directory):
        self.assertEqual(process.returncode, 1, process.stderr)
        self.assertIn(message, process.stderr)
        self.assertIn(REQUEST_ID, process.stderr)
        self.assertFalse((pathlib.Path(directory) / "aggregate-output" / "aggregate.json").exists())

    def test_complete_outcomes_pass_the_aggregate_gate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)
            process = self.run_aggregate(directory)
            self.assertEqual(process.returncode, 0, process.stderr)
            self.assertIn("complete_mutants=33 shards=3", process.stdout)
            report = json.loads(
                (directory / "aggregate-output" / "aggregate.json").read_text(encoding="utf-8"))
            self.assertEqual(report, {"request_id": REQUEST_ID, "revision": REVISION,
                                      "ok": True, "shards": 3, "mutants": 33})

    def test_missing_shard_artifact_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)
            shutil.rmtree(directory / "mutation-artifacts" / "mutants-shard-3-of-3")
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Missing or unexpected shard artifacts", directory)

    def test_duplicate_shard_index_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)
            artifacts = directory / "mutation-artifacts"
            target = artifacts / "mutants-shard-3-of-3"
            shutil.rmtree(target)
            shutil.copytree(artifacts / "mutants-shard-2-of-3", target)
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Unexpected or duplicate shard index", directory)

    def test_unrelated_revision_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)
            receipt_path = (directory / "mutation-artifacts" / "mutants-shard-1-of-3"
                            / "mutation-results" / "receipt.json")
            receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
            receipt["revision"] = "other-revision"
            receipt_path.write_text(json.dumps(receipt), encoding="utf-8")
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Incomplete or unrelated shard receipt", directory)

    def test_incomplete_receipt_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)
            receipt_path = (directory / "mutation-artifacts" / "mutants-shard-1-of-3"
                            / "mutation-results" / "receipt.json")
            receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
            receipt["ok"] = False
            receipt_path.write_text(json.dumps(receipt), encoding="utf-8")
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Incomplete or unrelated shard receipt", directory)

    def test_mismatched_workspace_inventory_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)
            list_path = (directory / "mutation-artifacts" / "mutants-shard-1-of-3"
                         / "mutation-results" / "list.json")
            mutants = json.loads(list_path.read_text(encoding="utf-8"))
            mutants[0]["name"] = mutants[1]["name"]
            list_path.write_text(json.dumps(mutants), encoding="utf-8")
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Shard workspace inventory differs", directory)

    def test_unrelated_subprocess_receipt_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)
            receipt_path = (directory / "mutation-artifacts" / "mutants-shard-1-of-3"
                            / "mutation-results" / "list-receipt.json")
            receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
            receipt["request_id"] = "{0}/shard/2/list".format(REQUEST_ID)
            receipt_path.write_text(json.dumps(receipt), encoding="utf-8")
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Unrelated subprocess receipt", directory)

    def test_unexpected_subprocess_command_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)
            receipt_path = (directory / "mutation-artifacts" / "mutants-shard-1-of-3"
                            / "mutation-results" / "receipt.json")
            receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
            receipt["baseline"]["argv"] = baseline_command()[:-1]
            receipt_path.write_text(json.dumps(receipt), encoding="utf-8")
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Unexpected subprocess command", directory)

    def test_timed_out_subprocess_receipt_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)
            receipt_path = (directory / "mutation-artifacts" / "mutants-shard-1-of-3"
                            / "mutation-results" / "receipt.json")
            receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
            receipt["mutation"] = subprocess_receipt(
                "{0}/shard/0/mutants".format(REQUEST_ID), mutation_command(0, 3),
                exit_code=3, timed_out=False)
            receipt_path.write_text(json.dumps(receipt), encoding="utf-8")
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Subprocess failed or timed out", directory)

    def test_tampered_inventory_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)
            inventory_path = directory / "mutation-artifacts" / "mutation-inventory" / "inventory.json"
            value = json.loads(inventory_path.read_text(encoding="utf-8"))
            value["matrix"]["include"][1]["label"] = 99
            inventory_path.write_text(json.dumps(value), encoding="utf-8")
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Unexpected mutation inventory or execution selector",
                                 directory)

    def outcomes_of(self, directory, index=0):
        return directory / "mutation-artifacts" / "mutants-shard-{0}-of-3".format(index + 1) \
            / "mutants.out" / "outcomes.json"

    def mutate_outcomes(self, directory, index, transform):
        path = self.outcomes_of(directory, index)
        value = json.loads(path.read_text(encoding="utf-8"))
        transform(value)
        path.write_text(json.dumps(value), encoding="utf-8")

    def test_incomplete_outcome_execution_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)
            self.mutate_outcomes(directory, 0, lambda value: value.update({"end_time": None}))
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Mutation execution is incomplete", directory)

    def test_wrong_tool_version_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)
            self.mutate_outcomes(directory, 0,
                                 lambda value: value.update({"cargo_mutants_version": "26.0.0"}))
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Unexpected mutation tool version", directory)

    def test_failed_baseline_outcome_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)

            def transform(value):
                value["outcomes"][0]["phase_results"][1]["process_status"] = {"Failure": 100}

            self.mutate_outcomes(directory, 0, transform)
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Mutation baseline phases differ", directory)

    def test_cargo_test_outcome_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)

            def transform(value):
                value["outcomes"][-1]["phase_results"][-1]["argv"] = [
                    "cargo", "test", "--profile=test", "--verbose", "--workspace"]

            self.mutate_outcomes(directory, 0, transform)
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Mutation test scope or profile differs", directory)

    def test_timed_out_outcome_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)

            def transform(value):
                row = value["outcomes"][-1]
                row["summary"] = "Timeout"
                row["phase_results"] = [
                    phase("Build", "Success"),
                    {"phase": "Test", "duration": 180.006194902, "process_status": "Timeout",
                     "argv": TEST_ARGV}]

            self.mutate_outcomes(directory, 0, transform)
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Mutation was missed, timed out or failed", directory)

    def test_missed_outcome_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)

            def transform(value):
                row = value["outcomes"][-1]
                row["summary"] = "MissedMutant"
                row["phase_results"] = [phase("Build", "Success"), phase("Test", "Success")]

            self.mutate_outcomes(directory, 0, transform)
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Mutation was missed, timed out or failed", directory)

    def test_duplicate_outcome_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)
            self.mutate_outcomes(directory, 0,
                                 lambda value: value["outcomes"].append(value["outcomes"][-1]))
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Duplicate mutation outcome", directory)

    def test_missing_identity_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)
            self.mutate_outcomes(directory, 0,
                                 lambda value: value["outcomes"].pop())
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Mutation outcomes do not cover the exact shard",
                                 directory)

    def test_unrelated_identity_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)

            def transform(value):
                row = value["outcomes"][-1]
                row["scenario"]["Mutant"]["name"] += " unknown"

            self.mutate_outcomes(directory, 0, transform)
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Mutation outcomes do not cover the exact shard",
                                 directory)

    def test_mismatched_counters_fail_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)
            self.mutate_outcomes(directory, 0,
                                 lambda value: value.update({"caught": value["caught"] + 1}))
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Mutation outcome counters differ", directory)

    def test_receipt_counts_mismatch_fails_the_aggregate(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            build_fixture(directory)
            receipt_path = (directory / "mutation-artifacts" / "mutants-shard-1-of-3"
                            / "mutation-results" / "receipt.json")
            receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
            receipt["counts"]["caught"] += 1
            receipt_path.write_text(json.dumps(receipt), encoding="utf-8")
            process = self.run_aggregate(directory)
            self.assert_rejected(process, "Shard receipt counts differ", directory)


if __name__ == "__main__":
    unittest.main()
