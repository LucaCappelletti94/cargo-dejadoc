import json
import os
import pathlib
import signal
import subprocess
import sys
import tempfile
import time
import unittest

import dependency_witnesses as witnesses
import dependency_constants_witnesses as constants


class WitnessHarnessTests(unittest.TestCase):
    def matrix(self, cases, request_id):
        return witnesses.run_matrix(
            cases, request_id=request_id, toolchain="stable", timeout=60
        )

    def test_empty_fixture_and_unbounded_deadlines_are_rejected(self):
        case = {"id": "bounds", "a": "fn main(){}", "b": "fn main(){}", "expected": "same"}
        for cases, timeout in (([], 60), ([case], float("inf")), ([case], float("nan"))):
            with self.subTest(timeout=timeout):
                with self.assertRaises(ValueError):
                    witnesses.run_matrix(
                        cases, request_id="DG-HARNESS-BOUNDS-01", timeout=timeout
                    )

    def test_matching_values_cover_independent_compiler_settings(self):
        case = {
            "id": "same_values",
            "a": 'fn main(){println!("{}",4u32 & 3u32);}',
            "b": 'fn main(){println!("{}",3u32 & 4u32);}',
            "expected": "same",
        }
        report = self.matrix([case], "DG-HARNESS-SAME-01")
        self.assertTrue(report["ok"], report)
        self.assertEqual(
            {(row["edition"], row["optimization"], row["overflow_checks"])
             for row in report["checks"]},
            {(edition, optimization, checked)
             for edition in ("2021", "2024")
             for optimization in (0, 2)
             for checked in (False, True)},
        )
        for result in report["results"]:
            self.assertEqual(result["run"]["stdout"], "0\n")
            self.assertEqual(result["compile"]["exit"], 0)
            self.assertEqual(result["run"]["exit"], 0)
            self.assertTrue(result["request_id"].startswith("DG-HARNESS-SAME-01/"))

    def test_overflow_expectation_depends_on_checks_and_not_optimization(self):
        cases = json.loads(
            pathlib.Path(__file__).with_name("fixtures").joinpath(
                "dependency", "cases.json"
            ).read_text()
        )
        case = next(row for row in cases if row["id"] == "N16_literal_overflow_association")
        report = self.matrix([case], "DG-HARNESS-OVERFLOW-01")
        self.assertTrue(report["ok"], report)
        for check in report["checks"]:
            self.assertEqual(check["equal"], not check["overflow_checks"])
            self.assertTrue(check["matches_expectation"])
        for result in report["results"]:
            expected_exit = 101 if result["side"] == "a" and result["overflow_checks"] else 0
            self.assertEqual(result["run"]["exit"], expected_exit)

    def test_equal_values_cannot_satisfy_a_difference_expectation(self):
        case = {
            "id": "incorrect_expectation",
            "a": 'fn main(){println!("{}",2u32 + 3u32);}',
            "b": 'fn main(){println!("{}",5u32);}',
            "expected": "different",
        }
        report = self.matrix([case], "DG-HARNESS-MISMATCH-01")
        self.assertFalse(report["ok"])
        for check in report["checks"]:
            self.assertTrue(check["compiled"])
            self.assertTrue(check["equal"])
            self.assertFalse(check["matches_expectation"])

    def test_constant_compiler_errors_cannot_establish_native_equality(self):
        case = {
            "id": "constant_compile_errors",
            "family": "constant",
            "a": 'const N:u32=5u32/0u32; fn main(){println!("{}",N);}',
            "b": 'const N:u32=6u32/0u32; fn main(){println!("{}",N);}',
            "expected": "same",
            "expect": {
                side: {
                    profile: {"compile": ["error[E0080]"], "run": "absent"}
                    for profile in ("off", "on")
                }
                for side in ("a", "b")
            },
        }
        report = constants.run_matrix(
            [case], request_id="DG-HARNESS-CONSTANT-ERRORS-01",
            toolchain="stable", timeout=60,
        )
        self.assertFalse(report["ok"])
        for check in report["checks"]:
            self.assertFalse(check["relation_matches"])

    def test_two_compiler_errors_cannot_count_as_equivalent_execution(self):
        case = {
            "id": "compile_errors",
            "a": "fn main(){unresolved_value();}",
            "b": "fn main(){unresolved_value();}",
            "expected": "same",
        }
        report = self.matrix([case], "DG-HARNESS-COMPILE-01")
        self.assertFalse(report["ok"])
        for result in report["results"]:
            self.assertNotEqual(result["compile"]["exit"], 0)
            self.assertIn("E0425", result["compile"]["stderr"])
            self.assertIsNone(result["run"])
        for check in report["checks"]:
            self.assertFalse(check["compiled"])
            self.assertFalse(check["matches_expectation"])

    def test_timeout_terminates_descendants_after_the_leader_exits(self):
        with tempfile.TemporaryDirectory() as directory:
            marker = pathlib.Path(directory) / "child.json"
            child_program = (
                "import json, os, pathlib, time; "
                f"pathlib.Path({str(marker)!r}).write_text(json.dumps("
                "{'request_id': 'DG-HARNESS-TIMEOUT-01', 'pid': os.getpid()})); "
                "print('observed', flush=True); time.sleep(60)"
            )
            parent_program = (
                f"import subprocess, sys; subprocess.Popen([sys.executable, '-c', {child_program!r}])"
            )
            deadline = time.monotonic() + 3
            result = witnesses.run_process(
                [sys.executable, "-c", parent_program],
                request_id="DG-HARNESS-TIMEOUT-01",
                deadline=deadline,
                timeout=0.5,
            )
            self.assertEqual(result["request_id"], "DG-HARNESS-TIMEOUT-01")
            self.assertTrue(result["timed_out"])
            self.assertIsNone(result["exit"])
            child = json.loads(marker.read_text())
            self.assertEqual(child["request_id"], result["request_id"])
            while True:
                try:
                    state = pathlib.Path(f"/proc/{child['pid']}/stat").read_text().rpartition(") ")[2].split()[0]
                except (FileNotFoundError, ProcessLookupError):
                    state = "exited"
                if state in ("Z", "X", "exited") or time.monotonic() >= deadline:
                    break
                time.sleep(min(0.01, max(0, deadline - time.monotonic())))
            if state not in ("Z", "X", "exited"):
                try:
                    os.kill(child["pid"], signal.SIGKILL)
                except ProcessLookupError:
                    pass
            self.assertIn(state, ("Z", "X", "exited"), "descendant survived the timeout")
            self.assertEqual(result["stdout"], "observed\n")

    def test_global_timeout_keeps_one_result_for_each_request(self):
        source = (
            "fn main(){std::thread::sleep(std::time::Duration::from_millis(500));"
            'println!("done");}'
        )
        report = witnesses.run_matrix(
            [{"id": "deadline", "a": source, "b": source, "expected": "same"}],
            request_id="DG-HARNESS-GLOBAL-01",
            toolchain="stable",
            timeout=3,
        )
        self.assertFalse(report["ok"])
        expected_ids = {
            f"DG-HARNESS-GLOBAL-01/deadline/{edition}/{optimization}/{int(checked)}/{side}"
            for edition in ("2021", "2024")
            for optimization in (0, 2)
            for checked in (False, True)
            for side in ("a", "b")
        }
        self.assertEqual({row["request_id"] for row in report["results"]}, expected_ids)
        self.assertEqual(len(report["results"]), len(expected_ids))

    def test_cli_writes_a_correlated_report_and_fails_on_bad_expectations(self):
        script = pathlib.Path(__file__).with_name("dependency_witnesses.py")
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            case_path = root / "cases.json"
            output = root / "report.json"
            case_path.write_text(json.dumps([{
                "id": "cli_mismatch",
                "a": 'fn main(){println!("same");}',
                "b": 'fn main(){println!("same");}',
                "expected": "different",
            }]))
            process = subprocess.run(
                [sys.executable, str(script), "--cases", str(case_path),
                 "--output", str(output), "--request-id", "DG-HARNESS-CLI-01",
                 "--toolchain", "stable", "--timeout", "30"],
                capture_output=True, text=True, timeout=40,
            )
            self.assertEqual(process.returncode, 1, process.stderr)
            report = json.loads(output.read_text())
            self.assertEqual(report["request_id"], "DG-HARNESS-CLI-01")
            self.assertFalse(report["ok"])
            self.assertIn("DG-HARNESS-CLI-01", process.stdout)


if __name__ == "__main__":
    unittest.main()
