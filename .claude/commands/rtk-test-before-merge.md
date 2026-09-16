Force the agent to write or extend a test for every behaviour change.

# Test before merge

For every code change that affects observable behaviour:

1. Identify the existing test that covers the area. If none, propose
   creating it (note the gap in the response).
2. Write the failing test FIRST in a separate commit named
   `test: <what>` — verify it fails on main, passes on the branch.
3. The implementation commit comes second.
4. If the change is purely refactoring, run the existing tests
   and report the count + duration; do not add new tests just to
   satisfy this rule when there is no behaviour change.

Skip is allowed only when:
- Tweaking a doc string, comment, log message
- Renaming a private symbol
- A pre-existing TODO points at "test added in #issue-N"
