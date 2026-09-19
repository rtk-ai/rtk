"""Tests for the transform_tool_result output filters in the RTK Hermes plugin."""

import importlib.util
import io
import json
import sys
import unittest
from pathlib import Path
from unittest import mock

PLUGIN_PATH = Path(__file__).resolve().parents[1] / "rtk-rewrite" / "__init__.py"


def load_plugin():
    spec = importlib.util.spec_from_file_location("rtk_rewrite_plugin", PLUGIN_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def make_terminal_result(output, exit_code=0):
    return json.dumps({"output": output, "exit_code": exit_code, "error": ""})


def make_browser_result(snapshot, **extra):
    data = {"snapshot": snapshot, "url": "https://example.com"}
    data.update(extra)
    return json.dumps(data)


# ---------------------------------------------------------------------------
# _transform_tool_result dispatch
# ---------------------------------------------------------------------------

class TransformDispatchTest(unittest.TestCase):
    def setUp(self):
        self.m = load_plugin()

    def test_unknown_tool_returns_none(self):
        self.assertIsNone(self.m._transform_tool_result(tool_name="unknown_tool", result="anything"))

    def test_none_result_returns_none(self):
        for tool in ("terminal", "browser_navigate", "read_file", "write_file", "patch", "search_files", "kanban_show"):
            with self.subTest(tool=tool):
                self.assertIsNone(self.m._transform_tool_result(tool_name=tool, result=None))

    def test_empty_result_returns_none(self):
        for tool in ("terminal", "browser_navigate", "read_file", "search_files"):
            with self.subTest(tool=tool):
                self.assertIsNone(self.m._transform_tool_result(tool_name=tool, result=""))


# ---------------------------------------------------------------------------
# _filter_text
# ---------------------------------------------------------------------------

class FilterTextTest(unittest.TestCase):
    def setUp(self):
        self.m = load_plugin()

    def test_deduplicates_repeated_lines(self):
        result = self.m._filter_text("foo\nfoo\nbar\nfoo\n")
        self.assertEqual("foo\nbar", result)

    def test_strips_blank_lines(self):
        result = self.m._filter_text("a\n\n\nb\n\nc\n")
        self.assertEqual("a\nb\nc", result)

    def test_truncates_at_100_lines(self):
        lines = "\n".join(f"line{i}" for i in range(150))
        result = self.m._filter_text(lines)
        out_lines = result.splitlines()
        self.assertEqual(101, len(out_lines))
        self.assertIn("truncated", out_lines[-1])

    def test_no_change_returns_none(self):
        text = "\n".join(f"unique{i}" for i in range(10))
        self.assertIsNone(self.m._filter_text(text))

    def test_whitespace_only_returns_none(self):
        self.assertIsNone(self.m._filter_text("   \n\n  "))

    def test_logs_reduction_to_stderr(self):
        with mock.patch.object(self.m.sys, "stderr", new_callable=io.StringIO) as err:
            self.m._filter_text("dup\ndup\ndup\n")
        self.assertIn("rtk: filtered text", err.getvalue())


# ---------------------------------------------------------------------------
# _filter_terminal
# ---------------------------------------------------------------------------

class FilterTerminalTest(unittest.TestCase):
    def setUp(self):
        self.m = load_plugin()

    def test_unwraps_json_and_filters_output(self):
        raw = make_terminal_result("foo\nfoo\nbar\n")
        result = self.m._filter_terminal(raw)
        self.assertIsNotNone(result)
        data = json.loads(result)
        self.assertEqual("foo\nbar", data["output"])

    def test_preserves_exit_code_and_error_fields(self):
        raw = json.dumps({"output": "dup\ndup\n", "exit_code": 1, "error": "oops"})
        result = self.m._filter_terminal(raw)
        data = json.loads(result)
        self.assertEqual(1, data["exit_code"])
        self.assertEqual("oops", data["error"])

    def test_empty_output_field_returns_none(self):
        raw = json.dumps({"output": "", "exit_code": 0, "error": ""})
        self.assertIsNone(self.m._filter_terminal(raw))

    def test_non_json_falls_back_to_filter_text(self):
        raw = "dup\ndup\nother\n"
        result = self.m._filter_terminal(raw)
        self.assertEqual("dup\nother", result)

    def test_no_change_returns_none(self):
        raw = make_terminal_result("\n".join(f"unique{i}" for i in range(5)))
        self.assertIsNone(self.m._filter_terminal(raw))

    def test_logs_reduction_to_stderr(self):
        raw = make_terminal_result("dup\ndup\n")
        with mock.patch.object(self.m.sys, "stderr", new_callable=io.StringIO) as err:
            self.m._filter_terminal(raw)
        self.assertIn("rtk: filtered terminal", err.getvalue())


# ---------------------------------------------------------------------------
# _filter_browser
# ---------------------------------------------------------------------------

class FilterBrowserTest(unittest.TestCase):
    def setUp(self):
        self.m = load_plugin()

    def test_strips_layout_noise_lines(self):
        snapshot = "LayoutBlock\n  heading Hello\nLayoutInline foo\n  link Click me\n"
        raw = make_browser_result(snapshot)
        result = self.m._filter_browser(raw)
        self.assertIsNotNone(result)
        data = json.loads(result)
        self.assertNotIn("LayoutBlock", data["snapshot"])
        self.assertIn("heading Hello", data["snapshot"])

    def test_deduplicates_snapshot_lines(self):
        snapshot = "heading Hello\nheading Hello\nlink Foo\n"
        raw = make_browser_result(snapshot)
        result = self.m._filter_browser(raw)
        data = json.loads(result)
        self.assertEqual(1, data["snapshot"].count("heading Hello"))

    def test_truncates_snapshot_at_120_lines(self):
        snapshot = "\n".join(f"link item{i}" for i in range(200))
        raw = make_browser_result(snapshot)
        result = self.m._filter_browser(raw)
        data = json.loads(result)
        lines = data["snapshot"].splitlines()
        self.assertEqual(121, len(lines))
        self.assertIn("truncated", lines[-1])

    def test_drops_stealth_warning_and_stealth_features(self):
        snapshot = "heading Hello\n"
        raw = json.dumps({
            "snapshot": snapshot,
            "stealth_warning": "some warning",
            "stealth_features": ["feature1"],
        })
        result = self.m._filter_browser(raw)
        data = json.loads(result)
        self.assertNotIn("stealth_warning", data)
        self.assertNotIn("stealth_features", data)

    def test_non_json_falls_back_to_filter_text(self):
        raw = "dup\ndup\nother\n"
        result = self.m._filter_browser(raw)
        self.assertEqual("dup\nother", result)

    def test_no_change_returns_none(self):
        snapshot = "\n".join(f"link item{i}" for i in range(5))
        raw = make_browser_result(snapshot)
        self.assertIsNone(self.m._filter_browser(raw))

    def test_logs_reduction_to_stderr(self):
        snapshot = "LayoutBlock noise\n" * 10 + "heading Real content\n"
        raw = make_browser_result(snapshot)
        with mock.patch.object(self.m.sys, "stderr", new_callable=io.StringIO) as err:
            self.m._filter_browser(raw)
        self.assertIn("rtk: filtered browser_navigate", err.getvalue())


# ---------------------------------------------------------------------------
# _filter_diff
# ---------------------------------------------------------------------------

class FilterDiffTest(unittest.TestCase):
    def setUp(self):
        self.m = load_plugin()

    SAMPLE_DIFF = "\n".join([
        "--- a/src/foo.py",
        "+++ b/src/foo.py",
        "@@ -1,4 +1,5 @@",
        " unchanged",
        "-removed line",
        "+added line 1",
        "+added line 2",
        "@@ -10,3 +11,2 @@",
        " ctx",
        "-old",
        "+new",
    ])

    def test_produces_compact_summary(self):
        result = self.m._filter_diff(self.SAMPLE_DIFF)
        self.assertIsNotNone(result)
        self.assertTrue(result.startswith("[patch]"))
        self.assertIn("src/foo.py", result)
        self.assertIn("+3", result)
        self.assertIn("-2", result)
        self.assertIn("2 hunks", result)

    def test_single_hunk_uses_singular(self):
        diff = "--- a/x.py\n+++ b/x.py\n@@ -1 +1 @@\n-old\n+new\n"
        result = self.m._filter_diff(diff)
        self.assertIn("1 hunk", result)
        self.assertNotIn("1 hunks", result)

    def test_shorter_than_original_returns_summary(self):
        result = self.m._filter_diff(self.SAMPLE_DIFF)
        self.assertLess(len(result), len(self.SAMPLE_DIFF))

    def test_very_short_diff_returns_none(self):
        # If summary is longer than original, no filtering needed
        diff = "[patch] x (1 hunk, +1/-0 lines)"
        self.assertIsNone(self.m._filter_diff(diff))

    def test_logs_reduction_to_stderr(self):
        with mock.patch.object(self.m.sys, "stderr", new_callable=io.StringIO) as err:
            self.m._filter_diff(self.SAMPLE_DIFF)
        self.assertIn("rtk: filtered diff", err.getvalue())

    def test_empty_returns_none(self):
        self.assertIsNone(self.m._filter_diff(""))
        self.assertIsNone(self.m._filter_diff("   "))


# ---------------------------------------------------------------------------
# _filter_search_files
# ---------------------------------------------------------------------------

class FilterSearchFilesTest(unittest.TestCase):
    def setUp(self):
        self.m = load_plugin()

    def test_truncates_above_50_results(self):
        lines = "\n".join(f"src/file{i}.py:10: match" for i in range(80))
        result = self.m._filter_search_files(lines)
        self.assertIsNotNone(result)
        out_lines = result.splitlines()
        self.assertEqual(51, len(out_lines))
        self.assertIn("30 results truncated", out_lines[-1])

    def test_under_limit_returns_none(self):
        lines = "\n".join(f"src/file{i}.py:1: match" for i in range(30))
        self.assertIsNone(self.m._filter_search_files(lines))

    def test_exactly_50_returns_none(self):
        lines = "\n".join(f"src/file{i}.py:1: match" for i in range(50))
        self.assertIsNone(self.m._filter_search_files(lines))

    def test_blank_lines_not_counted_as_results(self):
        lines = "\n".join(f"src/file{i}.py:1: match" for i in range(30))
        lines_with_blanks = lines + "\n\n\n"
        self.assertIsNone(self.m._filter_search_files(lines_with_blanks))

    def test_logs_reduction_to_stderr(self):
        lines = "\n".join(f"src/file{i}.py:1: match" for i in range(80))
        with mock.patch.object(self.m.sys, "stderr", new_callable=io.StringIO) as err:
            self.m._filter_search_files(lines)
        self.assertIn("rtk: filtered search_files", err.getvalue())


# ---------------------------------------------------------------------------
# _filter_kanban
# ---------------------------------------------------------------------------

class FilterKanbanTest(unittest.TestCase):
    def setUp(self):
        self.m = load_plugin()

    def _make_kanban(self, n_events=15):
        lines = [
            "Task: t_abc123 — Do something",
            "Status: running",
            "Body: some body text",
            f"Events ({n_events})",
        ]
        for i in range(n_events):
            lines.append(f"  [{i}] event_{i} — detail")
        lines += ["Runs (1)", "  run 1 — completed"]
        return "\n".join(lines)

    def test_truncates_events_above_10(self):
        raw = self._make_kanban(n_events=20)
        result = self.m._filter_kanban(raw)
        self.assertIsNotNone(result)
        self.assertIn("10 older events truncated", result)

    def test_under_10_events_returns_none(self):
        raw = self._make_kanban(n_events=8)
        self.assertIsNone(self.m._filter_kanban(raw))

    def test_exactly_10_events_returns_none(self):
        raw = self._make_kanban(n_events=10)
        self.assertIsNone(self.m._filter_kanban(raw))

    def test_preserves_non_event_sections(self):
        raw = self._make_kanban(n_events=20)
        result = self.m._filter_kanban(raw)
        self.assertIn("Body: some body text", result)
        self.assertIn("Runs (1)", result)
        self.assertIn("run 1 — completed", result)

    def test_logs_reduction_to_stderr(self):
        raw = self._make_kanban(n_events=20)
        with mock.patch.object(self.m.sys, "stderr", new_callable=io.StringIO) as err:
            self.m._filter_kanban(raw)
        self.assertIn("rtk: filtered kanban", err.getvalue())

    def test_empty_returns_none(self):
        self.assertIsNone(self.m._filter_kanban(""))

    def test_kanban_list_tool_is_also_filtered(self):
        raw = self._make_kanban(n_events=20)
        result = self.m._transform_tool_result(tool_name="kanban_list", result=raw)
        self.assertIsNotNone(result)
        self.assertIn("older events truncated", result)

    def test_kanban_show_tool_is_also_filtered(self):
        raw = self._make_kanban(n_events=20)
        result = self.m._transform_tool_result(tool_name="kanban_show", result=raw)
        self.assertIsNotNone(result)


# ---------------------------------------------------------------------------
# execute_code / read_file use _filter_text
# ---------------------------------------------------------------------------

class FilterExecuteCodeReadFileTest(unittest.TestCase):
    def setUp(self):
        self.m = load_plugin()

    def test_execute_code_deduplicates(self):
        raw = "line\nline\nother\n"
        result = self.m._transform_tool_result(tool_name="execute_code", result=raw)
        self.assertEqual("line\nother", result)

    def test_read_file_deduplicates(self):
        raw = "import os\nimport os\nimport sys\n"
        result = self.m._transform_tool_result(tool_name="read_file", result=raw)
        self.assertEqual("import os\nimport sys", result)

    def test_unique_content_returns_none(self):
        # No trailing newline, no blank lines, no duplicates — _filter_text has nothing to do.
        raw = "line_a\nline_b\nline_c"
        self.assertIsNone(self.m._transform_tool_result(tool_name="read_file", result=raw))


if __name__ == "__main__":
    unittest.main()
