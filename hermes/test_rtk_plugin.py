"""
Tests for the RTK Rewrite Plugin for Hermes.

Covers:
- RTK binary detection (available / not available)
- Command rewriting via `rtk rewrite`
- Hermes-specific skip guards
- Hook registration and invocation
- Edge cases: non-terminal tools, empty commands, timeouts, errors
- In-place args mutation
"""

import subprocess
from unittest.mock import MagicMock, patch

import pytest

# Import the plugin package (hermes/)
import hermes as rtk_plugin


@pytest.fixture(autouse=True)
def _reset_rtk_cache():
    """Reset the cached rtk availability check between tests."""
    rtk_plugin._rtk_available = None
    yield
    rtk_plugin._rtk_available = None


# ==========================================================================
# _check_rtk
# ==========================================================================

class TestCheckRtk:
    def test_rtk_found(self):
        with patch("shutil.which", return_value="/usr/local/bin/rtk"):
            assert rtk_plugin._check_rtk() is True

    def test_rtk_not_found(self):
        with patch("shutil.which", return_value=None):
            assert rtk_plugin._check_rtk() is False

    def test_result_is_cached(self):
        with patch("shutil.which", return_value="/usr/local/bin/rtk") as mock_which:
            rtk_plugin._check_rtk()
            rtk_plugin._check_rtk()
            mock_which.assert_called_once()


# ==========================================================================
# _should_skip
# ==========================================================================

class TestShouldSkip:
    def test_skips_explicit_rtk_command(self):
        assert rtk_plugin._should_skip("rtk git status") is True

    def test_skips_rtk_disabled_command(self):
        assert rtk_plugin._should_skip("RTK_DISABLED=1 git status") is True

    def test_does_not_skip_normal_command(self):
        assert rtk_plugin._should_skip("git status") is False


# ==========================================================================
# _try_rewrite
# ==========================================================================

class TestTryRewrite:
    def test_successful_rewrite_exit_code_0(self):
        fake_result = subprocess.CompletedProcess(
            args=["rtk", "rewrite", "git status"],
            returncode=0,
            stdout="rtk git status\n",
            stderr="",
        )
        with patch("subprocess.run", return_value=fake_result):
            assert rtk_plugin._try_rewrite("git status") == "rtk git status"

    def test_successful_rewrite_exit_code_3(self):
        fake_result = subprocess.CompletedProcess(
            args=["rtk", "rewrite", "git status"],
            returncode=3,
            stdout="rtk git status\n",
            stderr="",
        )
        with patch("subprocess.run", return_value=fake_result):
            assert rtk_plugin._try_rewrite("git status") == "rtk git status"

    def test_no_rewrite_same_command(self):
        """When rtk rewrite returns the same command, return None."""
        fake_result = subprocess.CompletedProcess(
            args=["rtk", "rewrite", "echo hello"],
            returncode=0,
            stdout="echo hello\n",
            stderr="",
        )
        with patch("subprocess.run", return_value=fake_result):
            assert rtk_plugin._try_rewrite("echo hello") is None

    def test_no_rewrite_exit_code_1(self):
        """Exit code 1 means no RTK equivalent."""
        fake_result = subprocess.CompletedProcess(
            args=["rtk", "rewrite", "some_custom_cmd"],
            returncode=1,
            stdout="",
            stderr="",
        )
        with patch("subprocess.run", return_value=fake_result):
            assert rtk_plugin._try_rewrite("some_custom_cmd") is None

    def test_no_rewrite_empty_stdout(self):
        fake_result = subprocess.CompletedProcess(
            args=["rtk", "rewrite", "git status"],
            returncode=0,
            stdout="",
            stderr="",
        )
        with patch("subprocess.run", return_value=fake_result):
            assert rtk_plugin._try_rewrite("git status") is None

    def test_timeout_returns_none(self):
        with patch("subprocess.run", side_effect=subprocess.TimeoutExpired("rtk", 2)):
            assert rtk_plugin._try_rewrite("git status") is None

    def test_file_not_found_returns_none(self):
        with patch("subprocess.run", side_effect=FileNotFoundError):
            assert rtk_plugin._try_rewrite("git status") is None

    def test_os_error_returns_none(self):
        with patch("subprocess.run", side_effect=OSError("broken")):
            assert rtk_plugin._try_rewrite("git status") is None

    def test_rewrite_strips_whitespace(self):
        fake_result = subprocess.CompletedProcess(
            args=["rtk", "rewrite", "ls -la"],
            returncode=3,
            stdout="  rtk ls -la  \n",
            stderr="",
        )
        with patch("subprocess.run", return_value=fake_result):
            assert rtk_plugin._try_rewrite("ls -la") == "rtk ls -la"

    def test_rewrite_passes_command_as_argument(self):
        """Verify the command is passed as a separate argument, not shell-expanded."""
        fake_result = subprocess.CompletedProcess(args=[], returncode=1, stdout="", stderr="")
        with patch("subprocess.run", return_value=fake_result) as mock_run:
            rtk_plugin._try_rewrite("git log --oneline -5")
            mock_run.assert_called_once_with(
                ["rtk", "rewrite", "git log --oneline -5"],
                capture_output=True,
                text=True,
                timeout=2,
            )

    def test_skip_guard_avoids_subprocess_for_explicit_rtk_command(self):
        with patch("subprocess.run") as mock_run:
            assert rtk_plugin._try_rewrite("rtk git status") is None
            mock_run.assert_not_called()

    def test_skip_guard_avoids_subprocess_for_rtk_disabled_command(self):
        with patch("subprocess.run") as mock_run:
            assert rtk_plugin._try_rewrite("RTK_DISABLED=1 git status") is None
            mock_run.assert_not_called()


# ==========================================================================
# _pre_tool_call hook
# ==========================================================================

class TestPreToolCall:
    def test_rewrites_terminal_command(self):
        """Hook should mutate args in-place when a rewrite is available."""
        args = {"command": "git status"}
        with patch.object(rtk_plugin, "_try_rewrite", return_value="rtk git status"):
            rtk_plugin._pre_tool_call(tool_name="terminal", args=args, task_id="test")
        assert args["command"] == "rtk git status"

    def test_ignores_non_terminal_tools(self):
        """Hook should skip tools that aren't 'terminal'."""
        args = {"command": "git status"}
        with patch.object(rtk_plugin, "_try_rewrite") as mock_rewrite:
            rtk_plugin._pre_tool_call(tool_name="web_search", args=args, task_id="test")
            mock_rewrite.assert_not_called()
        assert args["command"] == "git status"

    def test_ignores_missing_command(self):
        """Hook should skip if args has no 'command' key."""
        args = {"background": True}
        with patch.object(rtk_plugin, "_try_rewrite") as mock_rewrite:
            rtk_plugin._pre_tool_call(tool_name="terminal", args=args, task_id="test")
            mock_rewrite.assert_not_called()

    def test_ignores_non_string_command(self):
        """Hook should skip if command is not a string."""
        args = {"command": 123}
        with patch.object(rtk_plugin, "_try_rewrite") as mock_rewrite:
            rtk_plugin._pre_tool_call(tool_name="terminal", args=args, task_id="test")
            mock_rewrite.assert_not_called()

    def test_ignores_empty_command(self):
        """Hook should skip empty command strings."""
        args = {"command": "   "}
        with patch.object(rtk_plugin, "_try_rewrite") as mock_rewrite:
            rtk_plugin._pre_tool_call(tool_name="terminal", args=args, task_id="test")
            mock_rewrite.assert_not_called()

    def test_no_mutation_when_no_rewrite(self):
        """Hook should not modify args when rtk returns None."""
        args = {"command": "echo hello"}
        with patch.object(rtk_plugin, "_try_rewrite", return_value=None):
            rtk_plugin._pre_tool_call(tool_name="terminal", args=args, task_id="test")
        assert args["command"] == "echo hello"

    def test_preserves_other_args(self):
        """Hook should only modify 'command', leaving other args untouched."""
        args = {"command": "git status", "timeout": 30, "workdir": "/tmp"}
        with patch.object(rtk_plugin, "_try_rewrite", return_value="rtk git status"):
            rtk_plugin._pre_tool_call(tool_name="terminal", args=args, task_id="test")
        assert args == {"command": "rtk git status", "timeout": 30, "workdir": "/tmp"}

    def test_handles_extra_kwargs(self):
        """Hook should accept and ignore extra keyword arguments."""
        args = {"command": "git status"}
        with patch.object(rtk_plugin, "_try_rewrite", return_value="rtk git status"):
            rtk_plugin._pre_tool_call(
                tool_name="terminal", args=args, task_id="test", unexpected="value"
            )
        assert args["command"] == "rtk git status"


# ==========================================================================
# register()
# ==========================================================================

class TestRegister:
    def test_registers_hook_when_rtk_available(self):
        ctx = MagicMock()
        with patch.object(rtk_plugin, "_check_rtk", return_value=True):
            rtk_plugin.register(ctx)
        ctx.register_hook.assert_called_once_with("pre_tool_call", rtk_plugin._pre_tool_call)

    def test_skips_registration_when_rtk_missing(self):
        ctx = MagicMock()
        with patch.object(rtk_plugin, "_check_rtk", return_value=False):
            rtk_plugin.register(ctx)
        ctx.register_hook.assert_not_called()

    def test_register_does_not_raise_on_missing_rtk(self):
        """Plugin should degrade gracefully, never crash the agent."""
        ctx = MagicMock()
        with patch.object(rtk_plugin, "_check_rtk", return_value=False):
            rtk_plugin.register(ctx)  # Should not raise


# ==========================================================================
# Integration-style tests (no real rtk binary)
# ==========================================================================

class TestIntegration:
    def test_full_flow_rewrite_exit_code_3(self):
        """Simulate the full flow: register -> hook fires -> command rewritten."""
        registered_hooks = {}

        class FakeCtx:
            def register_hook(self, name, callback):
                registered_hooks[name] = callback

        with patch.object(rtk_plugin, "_check_rtk", return_value=True):
            rtk_plugin.register(FakeCtx())

        assert "pre_tool_call" in registered_hooks

        args = {"command": "cargo test --nocapture"}
        fake_result = subprocess.CompletedProcess(
            args=[],
            returncode=3,
            stdout="rtk cargo test --nocapture\n",
            stderr="",
        )
        with patch("subprocess.run", return_value=fake_result):
            registered_hooks["pre_tool_call"](
                tool_name="terminal", args=args, task_id="t1"
            )

        assert args["command"] == "rtk cargo test --nocapture"

    def test_full_flow_no_rewrite(self):
        """Simulate the full flow when rtk has no rewrite for a command."""
        registered_hooks = {}

        class FakeCtx:
            def register_hook(self, name, callback):
                registered_hooks[name] = callback

        with patch.object(rtk_plugin, "_check_rtk", return_value=True):
            rtk_plugin.register(FakeCtx())

        args = {"command": "echo hello"}
        fake_result = subprocess.CompletedProcess(args=[], returncode=1, stdout="", stderr="")
        with patch("subprocess.run", return_value=fake_result):
            registered_hooks["pre_tool_call"](
                tool_name="terminal", args=args, task_id="t1"
            )

        assert args["command"] == "echo hello"

    def test_full_flow_rtk_crashes(self):
        """If rtk binary crashes, command should pass through unchanged."""
        registered_hooks = {}

        class FakeCtx:
            def register_hook(self, name, callback):
                registered_hooks[name] = callback

        with patch.object(rtk_plugin, "_check_rtk", return_value=True):
            rtk_plugin.register(FakeCtx())

        args = {"command": "git status"}
        with patch("subprocess.run", side_effect=OSError("segfault")):
            registered_hooks["pre_tool_call"](
                tool_name="terminal", args=args, task_id="t1"
            )

        assert args["command"] == "git status"
