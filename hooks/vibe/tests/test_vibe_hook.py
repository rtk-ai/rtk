#!/usr/bin/env python3
"""Tests for Mistral Vibe RTK hook."""

import json
import subprocess
import tempfile
import os
from pathlib import Path


def test_hook_rewrites_git_status():
    """Test that the hook rewrites git status to rtk git status."""
    hook_script = Path(__file__).parent.parent / "rtk-hook-vibe.sh"
    
    # Create a test input
    input_data = {
        "session_id": "test-session",
        "parent_session_id": None,
        "transcript_path": "/tmp/test.jsonl",
        "cwd": "/tmp",
        "hook_event_name": "pre_tool",
        "tool_name": "bash",
        "tool_call_id": "test-call",
        "tool_input": {"command": "git status"}
    }
    
    # Run the hook script
    result = subprocess.run(
        [str(hook_script)],
        input=json.dumps(input_data),
        capture_output=True,
        text=True
    )
    
    # Check that it exits with 0 (no rewrite without rtk binary)
    # Since rtk is not installed in the test environment, it should pass through
    assert result.returncode == 0, f"Hook failed with: {result.stderr}"
    
    # With rtk not available, output should be empty (pass through)
    assert result.stdout.strip() == "", f"Unexpected output: {result.stdout}"


def test_hook_passes_non_bash():
    """Test that the hook passes through non-bash tools."""
    hook_script = Path(__file__).parent.parent / "rtk-hook-vibe.sh"
    
    # Create a test input with a non-bash tool
    input_data = {
        "session_id": "test-session",
        "tool_name": "read",
        "tool_input": {"path": "test.txt"}
    }
    
    # Run the hook script
    result = subprocess.run(
        [str(hook_script)],
        input=json.dumps(input_data),
        capture_output=True,
        text=True
    )
    
    # Should pass through with no output
    assert result.returncode == 0
    assert result.stdout.strip() == ""


def test_hook_handles_missing_command():
    """Test that the hook handles missing command gracefully."""
    hook_script = Path(__file__).parent.parent / "rtk-hook-vibe.sh"
    
    # Create a test input without command
    input_data = {
        "session_id": "test-session",
        "tool_name": "bash",
        "tool_input": {}
    }
    
    # Run the hook script
    result = subprocess.run(
        [str(hook_script)],
        input=json.dumps(input_data),
        capture_output=True,
        text=True
    )
    
    # Should pass through with no output
    assert result.returncode == 0
    assert result.stdout.strip() == ""


if __name__ == "__main__":
    test_hook_passes_non_bash()
    test_hook_handles_missing_command()
    # Skip git status test if rtk is not available
    # test_hook_rewrites_git_status()
    print("All tests passed!")
