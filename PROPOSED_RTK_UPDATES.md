Proposed RTK updates for OpenCode integration

Context
- Request: "RTK updates for OpenCode Repository https://github.com/rtk-ai/rtk"
- Environment: local machine lacks Cargo; cannot run cargo tests locally. Proceeding by proposing minimal, safe changes for maintainers to review and CI to validate.

Summary of proposed changes
1) CI: Ensure GitHub Actions run cargo build/test on stable toolchain (matrix), enable cargo cache, and add a lightweight Windows runner.
2) Docs: Add a short "OpenCode integration" section in README with usage examples (rtk init -g, rtk cargo test) and troubleshooting notes for Windows/PowerShell/WSL.
3) Tests: Add or enable a smoke test workflow that runs `cargo test --all --locked` on PRs and main branches.
4) Releases: Improve release workflow to publish pre-built binaries and update Homebrew formula automatically.

Implementation plan
- Branch: rtk-open-code-updates
- Changes: add PROPOSED_RTK_UPDATES.md (this file), update README.md with small integration section, modify .github/workflows/ci.yml to include cargo test matrix and cache.
- Submit PR for maintainer review; CI will run on PR and validate changes.

Notes
- Could not run tests locally (Cargo not available). Changes are focused on docs + CI so maintainers can validate via GitHub Actions.
- If permitted, follow-up will implement the README + CI edits and open the PR.

If this plan looks good, next step is to create branch, implement README+CI edits, push branch, and open a PR.
