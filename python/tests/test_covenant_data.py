"""The wheel's contract, and the Python half of the cross-plane golden test.

The engine promises the same verdict wherever it runs. Here the wheel is held
to the ``covenant`` command on the repository's example data: records checked
through the module must give the report the command gives for the same file.
Arrow data — pyarrow and polars, when installed — must reach the same
verdicts through the Arrow engine.

Runs against the installed wheel, not the source tree::

    cd python
    maturin build --release && pip install target/wheels/covenant_data-*.whl
    pip install pyarrow polars      # optional: the Arrow tests skip without them
    python -m unittest discover -s tests -v

Standard library only, apart from the optional pyarrow and polars.
"""

import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

import covenant_data
from covenant_data.testing import assert_conforms

try:
    import pyarrow as pa
except ImportError:  # pragma: no cover - depends on the environment
    pa = None

try:
    import polars as pl
except ImportError:  # pragma: no cover - depends on the environment
    pl = None

EXAMPLES = pathlib.Path(__file__).resolve().parents[2] / "examples"
CONTRACT = EXAMPLES / "contracts" / "orders.yaml"
CLEAN = EXAMPLES / "data" / "orders.ndjson"
DIRTY = EXAMPLES / "data" / "orders_bad.ndjson"
COMMAND = pathlib.Path(sys.executable).parent / "covenant"


def records(path):
    """The JSON objects of an NDJSON file; a line that is not JSON has no record form."""
    out = []
    for line in path.read_text().splitlines():
        try:
            out.append(json.loads(line))
        except ValueError:
            pass
    return out


def run(*args):
    return subprocess.run([str(COMMAND), *map(str, args)], capture_output=True, text=True)


def rule_counts(report):
    """A report's per-rule counts as a mapping: the list's order follows the batches."""
    report = report if isinstance(report, dict) else report.to_dict()
    return {(r["field"], r["rule"]): r["count"] for r in report["per_rule"]}


def order(order_id, amount=100, currency="EUR"):
    return {
        "order_id": order_id,
        "amount_cents": amount,
        "currency": currency,
        "created_at": "2026-09-24T10:00:00Z",
    }


class GoldenTest(unittest.TestCase):
    def test_records_get_the_report_the_command_gives_for_the_file(self):
        rows = records(DIRTY)
        with tempfile.TemporaryDirectory() as tmp:
            file = pathlib.Path(tmp) / "orders.ndjson"
            file.write_text("".join(json.dumps(r) + "\n" for r in rows))
            cli = run("check", file, "-c", CONTRACT, "--format", "json")
            self.assertEqual(cli.returncode, 1, cli.stderr)
            expected = json.loads(cli.stdout)[0]
            report = covenant_data.check(rows, CONTRACT, source=str(file))
        # Compared as parsed JSON: layout and key order are not the contract.
        self.assertEqual(report.to_dict(), expected)
        self.assertFalse(report.passed)

    def test_a_path_is_checked_as_the_command_checks_it(self):
        cli = json.loads(run("check", DIRTY, "-c", CONTRACT, "--format", "json").stdout)[0]
        self.assertEqual(covenant_data.check(DIRTY, CONTRACT).to_dict(), cli)
        self.assertTrue(covenant_data.check(str(CLEAN), CONTRACT).passed)


class ReportTest(unittest.TestCase):
    def test_a_report_reads_as_the_command_prints_it(self):
        contract = covenant_data.Contract(CONTRACT)
        report = contract.check(records(DIRTY))
        self.assertEqual((report.rows, report.violations, report.partial), (5, 8, False))
        self.assertFalse(report)
        self.assertIn("FAIL", str(report))
        self.assertEqual(json.loads(report.to_json()), report.to_dict())
        self.assertEqual(repr(report), "<Report FAIL — 5 rows, 8 violations>")
        self.assertEqual(repr(contract), "<Contract orders v1.2.0, model orders>")
        self.assertEqual((contract.id, contract.version, contract.model), ("orders", "1.2.0", "orders"))
        clean = contract.check(records(CLEAN))
        self.assertTrue(clean.passed and clean)
        self.assertEqual(clean.to_dict()["source"], "<records>")

    def test_a_warn_only_contract_passes_with_violations_as_the_command_does(self):
        text = CONTRACT.read_text().replace("on_violation: block", "on_violation: warn")
        contract = covenant_data.Contract.from_text(text, name="warn.yaml")
        report = contract.check(records(DIRTY))
        self.assertTrue(report.passed)
        self.assertEqual(report.violations, 8)
        with tempfile.TemporaryDirectory() as tmp:
            warn = pathlib.Path(tmp) / "warn.yaml"
            warn.write_text(text)
            self.assertEqual(run("check", DIRTY, "-c", warn).returncode, 0)


class ErrorTest(unittest.TestCase):
    ODCS = (
        "apiVersion: v3.1.0\nkind: DataContract\nid: c\nversion: 1.0.0\nschema:\n"
        "  - name: t\n    properties:\n      - name: a\n        logicalType: string\n"
        "        required: true\n      - name: b\n        logicalType: array\n"
    )

    def test_a_contract_that_cannot_be_read_is_an_error_not_a_verdict(self):
        with self.assertRaises(covenant_data.CovenantError) as caught:
            covenant_data.Contract(EXAMPLES / "contracts" / "missing.yaml")
        self.assertIn("missing.yaml", str(caught.exception))

    def test_a_partial_contract_is_refused_unless_allowed_and_then_says_so(self):
        with self.assertRaises(covenant_data.CovenantError) as caught:
            covenant_data.Contract.from_text(self.ODCS, name="c.odcs.yaml")
        self.assertIn("schema.t.properties.b", str(caught.exception))
        contract = covenant_data.Contract.from_text(self.ODCS, allow_unenforced=True)
        self.assertEqual(contract.unenforced[0]["path"], "schema.t.properties.b")
        report = contract.check([{"a": "x"}, {"a": None}])
        self.assertTrue(report.partial)
        self.assertEqual(report.violations, 1)
        self.assertIn("(partial)", repr(report))

    def test_what_cannot_be_checked_is_a_type_error(self):
        for data in (42, {"order_id": "x"}):
            with self.assertRaises(TypeError):
                covenant_data.check(data, CONTRACT)
        with self.assertRaises(TypeError):
            covenant_data.check(CLEAN, CONTRACT, source="name")
        with self.assertRaises(TypeError):
            covenant_data.check([], covenant_data.Contract(CONTRACT), model="orders")

    def test_nan_has_no_json_form_and_is_refused(self):
        with self.assertRaises(ValueError):
            covenant_data.check([{"amount_cents": float("nan")}], CONTRACT)


class ReportDocumentTest(unittest.TestCase):
    """The document a check writes to leave the machine: covenant-report/v1."""

    def test_a_check_writes_the_command_s_document_on_the_python_plane(self):
        doc = covenant_data.check(DIRTY, CONTRACT).report_document()
        self.assertEqual((doc["report"], doc["kind"], doc["verdict"]), (1, "check", "fail"))
        self.assertEqual(doc["run"]["plane"], "python")
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / "report.json"
            self.assertEqual(run("check", DIRTY, "-c", CONTRACT, "--report-json", path).returncode, 1)
            command = json.loads(path.read_text())
        # The same document but for its plane and its timings.
        self.assertEqual({**doc, "run": None}, {**command, "run": None})

    def test_samples_never_carry_a_value_and_none_keeps_the_counts(self):
        report = covenant_data.check([order("ord_000000000001", currency="XAU")], CONTRACT)
        self.assertIn("XAU", str(report))
        doc = report.report_document()
        self.assertNotIn("XAU", json.dumps(doc))
        self.assertEqual(
            doc["checks"][0]["samples"],
            [{"field": "currency", "rule": "allowed", "row": 0, "type": "string", "length": 3}],
        )
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / "report.json"
            self.assertIsNone(report.write_report(path, samples="none"))
            written = json.loads(path.read_text())
        self.assertEqual(written["sample_mode"], "none")
        self.assertEqual(written["checks"][0]["samples"], [])
        self.assertEqual(written["checks"][0]["per_rule"], doc["checks"][0]["per_rule"])

    def test_a_warn_only_contract_reports_warn(self):
        text = CONTRACT.read_text().replace("on_violation: block", "on_violation: warn")
        report = covenant_data.Contract.from_text(text, name="warn.yaml").check(records(DIRTY))
        self.assertTrue(report.passed)
        self.assertEqual(report.report_document()["verdict"], "warn")

    def test_hashed_samples_need_a_key_and_say_the_same_value_alike(self):
        report = covenant_data.check(
            [order("ord_000000000001", currency="XAU"), order("ord_000000000002", currency="XAU")],
            CONTRACT,
        )
        saved = os.environ.pop("COVENANT_SAMPLE_KEY", None)
        try:
            with self.assertRaises(covenant_data.CovenantError):
                report.report_document(samples="hashed")
            os.environ["COVENANT_SAMPLE_KEY"] = "4f1c0d9e8b7a6f5e4d3c2b1a0f9e8d7c"
            doc = report.report_document(samples="hashed")
        finally:
            os.environ.pop("COVENANT_SAMPLE_KEY", None)
            if saved is not None:
                os.environ["COVENANT_SAMPLE_KEY"] = saved
        self.assertEqual(doc["sample_mode"], "hashed")
        self.assertEqual(len(doc["sample_key"]), 16)
        first, second = doc["checks"][0]["samples"]
        self.assertEqual(first["hash"], second["hash"])
        self.assertNotIn("XAU", json.dumps(doc))

    def test_samples_is_masked_or_none(self):
        report = covenant_data.check(CLEAN, CONTRACT)
        with self.assertRaises(ValueError):
            report.report_document(samples="values")
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(covenant_data.CovenantError):
                report.write_report(pathlib.Path(tmp) / "no-such-dir" / "report.json")


class AssertConformsTest(unittest.TestCase):
    def test_failing_data_fails_the_test_with_the_report(self):
        with self.assertRaises(AssertionError) as caught:
            assert_conforms(records(DIRTY), CONTRACT)
        self.assertIn("amount_cents", str(caught.exception))
        self.assertTrue(assert_conforms(records(CLEAN), CONTRACT).passed)


class CommandTest(unittest.TestCase):
    def test_the_wheel_installs_the_covenant_command(self):
        self.assertEqual(run("validate", CONTRACT).returncode, 0)
        self.assertEqual(run("check", CLEAN, "-c", CONTRACT).returncode, 0)
        self.assertEqual(run("check", DIRTY, "-c", CONTRACT).returncode, 1)
        version = run("--version")
        self.assertEqual(version.returncode, 0)
        self.assertIn(covenant_data.__version__, version.stdout)
        self.assertEqual(run("no-such-command").returncode, 2)


@unittest.skipUnless(pa, "pyarrow is not installed")
class PyArrowTest(unittest.TestCase):
    def table(self, rows):
        return pa.Table.from_pylist(rows)

    def test_a_table_is_checked_zero_copy_through_the_arrow_engine(self):
        report = covenant_data.check(self.table(records(CLEAN)), CONTRACT)
        self.assertTrue(report.passed, str(report))
        self.assertEqual(report.rows, 3)
        self.assertEqual(report.to_dict()["source"], "pyarrow.lib.Table")

    def test_violations_and_uniqueness_across_batches(self):
        first = pa.RecordBatch.from_pylist([order("ord_aaaaaaaaaaaa"), order("bad", amount=-5)])
        second = pa.RecordBatch.from_pylist([order("ord_aaaaaaaaaaaa", currency="JPY")])
        reader = pa.RecordBatchReader.from_batches(first.schema, [first, second])
        report = covenant_data.check(reader, CONTRACT, source="orders batches")
        rules = {(r["field"], r["rule"]): r["count"] for r in report.to_dict()["per_rule"]}
        self.assertEqual(
            rules,
            {
                ("order_id", "pattern"): 1,
                ("amount_cents", "min"): 1,
                ("order_id", "unique"): 1,
                ("currency", "allowed"): 1,
            },
        )
        self.assertEqual(report.rows, 3)
        self.assertEqual(report.to_dict()["source"], "orders batches")

    def test_an_object_that_exports_one_array_is_checked_too(self):
        batch = pa.RecordBatch.from_pylist([order("ord_bbbbbbbbbbbb"), order("ord_bbbbbbbbbbbb")])

        class OnlyArray:
            def __arrow_c_array__(self, requested_schema=None):
                return batch.__arrow_c_array__(requested_schema)

        report = covenant_data.check(OnlyArray(), CONTRACT)
        self.assertEqual(report.violations, 1)

    def test_a_table_with_no_rows_is_still_held_to_its_schema(self):
        empty = pa.table({"order_id": pa.array([], pa.string())})
        report = covenant_data.check(empty, CONTRACT)
        self.assertFalse(report.passed)
        # Three required fields are missing, each counted once.
        self.assertEqual((report.rows, report.violations), (0, 3))
        # A stream that carries one zero-row batch gets the same verdict.
        batch = pa.RecordBatch.from_pydict({"order_id": pa.array([], pa.string())})
        reader = pa.RecordBatchReader.from_batches(batch.schema, [batch])
        streamed = covenant_data.check(reader, CONTRACT)
        self.assertEqual(streamed.to_dict()["per_rule"], report.to_dict()["per_rule"])

    def test_a_stream_in_several_batches_reports_what_one_batch_reports(self):
        # Three required fields missing, each once however the stream is
        # batched, and a bad id in the last batch.
        table = pa.table({"order_id": [f"ord_{i:012x}" for i in range(6)] + ["bad"]})
        batches = table.to_batches(max_chunksize=2)
        self.assertEqual(len(batches), 4)
        one = covenant_data.check(table, CONTRACT)
        several = covenant_data.check(pa.RecordBatchReader.from_batches(table.schema, batches), CONTRACT)
        self.assertEqual(rule_counts(several), rule_counts(one))
        self.assertEqual((several.violations, several.rows), (one.violations, one.rows))
        self.assertEqual(one.violations, 4)

    def test_a_parquet_file_gets_the_command_s_report_as_a_path_and_as_a_table(self):
        import pyarrow.parquet as pq

        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / "orders.parquet"
            ids = [f"ord_{i:012x}" for i in range(20_000)]
            pq.write_table(pa.table({"order_id": ids}), path, row_group_size=5_000)
            cli = run("check", path, "-c", CONTRACT, "--format", "json")
            self.assertEqual(cli.returncode, 1, cli.stderr)
            expected = json.loads(cli.stdout)[0]
            table = pq.read_table(path)
            self.assertGreater(table.column("order_id").num_chunks, 1)
            self.assertEqual(covenant_data.check(path, CONTRACT).to_dict(), expected)
            from_table = covenant_data.check(table, CONTRACT)
        self.assertEqual(rule_counts(from_table), rule_counts(expected))
        self.assertEqual(from_table.violations, expected["violations"])
        self.assertEqual(expected["violations"], 3)


@unittest.skipUnless(pl, "polars is not installed")
class PolarsTest(unittest.TestCase):
    def test_a_polars_frame_gets_the_verdict_its_records_get(self):
        for path in (CLEAN, DIRTY):
            rows = [r for r in records(path) if isinstance(r.get("amount_cents"), int)]
            frame = pl.DataFrame(rows, infer_schema_length=None)
            from_frame = covenant_data.check(frame, CONTRACT)
            from_records = covenant_data.check(rows, CONTRACT)
            self.assertEqual(from_frame.passed, from_records.passed, str(from_frame))
            self.assertEqual(from_frame.rows, len(rows))

    def test_a_frame_in_several_chunks_reports_what_one_chunk_reports(self):
        # polars 1.x exports a chunked frame as one batch (the pyarrow tests
        # cover several); whichever way a version exports it, the report is
        # the rechunked frame's.
        ids = [f"ord_{i:012x}" for i in range(6)]
        parts = [pl.DataFrame({"order_id": ids[:3]}), pl.DataFrame({"order_id": ids[3:] + ["bad"]})]
        chunked = pl.concat(parts, rechunk=False)
        self.assertGreater(chunked.n_chunks(), 1)
        one = covenant_data.check(chunked.rechunk(), CONTRACT)
        several = covenant_data.check(chunked, CONTRACT)
        self.assertEqual(rule_counts(several), rule_counts(one))
        self.assertEqual((several.violations, several.rows), (one.violations, one.rows))
        self.assertEqual(one.violations, 4)

    def test_an_empty_frame_is_held_to_its_schema_once(self):
        # polars exports an empty frame as one zero-row batch.
        frame = pl.DataFrame({"order_id": pl.Series([], dtype=pl.Utf8)})
        report = covenant_data.check(frame, CONTRACT)
        self.assertEqual((report.passed, report.rows, report.violations), (False, 0, 3))


if __name__ == "__main__":
    unittest.main()
