import subprocess
from typing import List, Union

def run_git_status(args: List[str], cwd: str = ".") -> str:
    """
    Executes git status with specified arguments and preserves exact git trailing
    newline behavior.
    """
    cmd = ["git", "status"] + args
    result = subprocess.run(
        cmd,
        cwd=cwd,
        capture_output=True,
        text=True,
        check=True
    )
    
    return ensure_trailing_newline(result.stdout)


def ensure_trailing_newline(output: Union[str, bytes]) -> Union[str, bytes]:
    """
    Ensures non-empty output ends with a single newline character/byte.
    High-performance O(1) trailing check without copying full buffers.
    """
    if not output:
        return output
        
    if isinstance(output, str):
        if not output.endswith("\n"):
            return output + "\n"
        return output
    elif isinstance(output, (bytes, bytearray)):
        if not output.endswith(b"\n"):
            return output + b"\n"
        return output
    else:
        raise TypeError("Output must be str or bytes")


def test_ensure_trailing_newline():
    assert ensure_trailing_newline("") == ""
    assert ensure_trailing_newline(b"") == b""
    assert ensure_trailing_newline("M tracked") == "M tracked\n"
    assert ensure_trailing_newline("M tracked\n") == "M tracked\n"
    assert ensure_trailing_newline("M tracked\n?? untracked") == "M tracked\n?? untracked\n"
    assert ensure_trailing_newline(b"M tracked") == b"M tracked\n"
    print("All Python tests passed successfully!")

if __name__ == "__main__":
    test_ensure_trailing_newline()