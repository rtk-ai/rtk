import json, os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ledger

def main():
    session = os.environ.get("CLAUDE_SESSION_ID") or (sys.argv[1] if len(sys.argv) > 1 else "")
    if not session:
        import glob
        paths = sorted(glob.glob(os.path.expanduser("~/.claude/projects/*/*.jsonl")), key=os.path.getmtime)
        session = os.path.basename(paths[-1])[:-6] if paths else "task"
    print("---ledger---")
    print(json.dumps(ledger.build(session)))
    print("---")

if __name__ == "__main__":
    main()
