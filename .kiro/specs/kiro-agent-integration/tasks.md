# Implementation Plan: Kiro Agent Integration

## Overview

Add Kiro IDE as a supported agent target in RTK's `init` command. The integration installs two files — a steering file (`.kiro/steering/rtk-rules.md`) and a preToolUse hook (`.kiro/hooks/rtk-shell-rewrite.kiro.hook`) — following the existing rules-file integration pattern but with dual-file transactional semantics. Implementation is in Rust, matching the existing codebase patterns in `src/hooks/init.rs` and `src/main.rs`.

## Tasks

- [x] 1. Create template files and register the Kiro agent target
  - [x] 1.1 Create the Kiro steering file template at `hooks/kiro/rules.md`
    - Create directory `hooks/kiro/`
    - Write `rules.md` with YAML frontmatter (`---\ninclusion: auto\n---`), level-1 heading containing "Kiro", Usage summary, Rule section, Examples subsection with at least 6 fenced code block examples (ls, grep, find, git, cargo, docker), When NOT to use section, Meta Commands section, and Why section
    - Match the section structure of `hooks/windsurf/rules.md` but include the frontmatter block
    - _Requirements: 1.2, 1.3, 1.4, 1.5, 1.6, 7.1, 7.2, 7.3, 7.4, 9.3_

  - [x] 1.2 Create the Kiro hook file template at `hooks/kiro/hook.json`
    - Write valid JSON with fields: `enabled` (true), `name` (string), `description` (string), `version` ("1"), `when` object with `type: "preToolUse"` and `toolTypes: ["shell"]`, `then` object with `type: "askAgent"` and `prompt` containing rewrite instructions
    - The prompt must instruct the agent to check for rtk prefix, rewrite supported commands, and skip interactive/already-prefixed/unsupported commands
    - _Requirements: 2.2, 2.3, 2.4, 2.5, 2.6_

  - [x] 1.3 Add `Kiro` variant to the `AgentTarget` enum in `src/main.rs`
    - Add `/// Kiro IDE` doc comment and `Kiro` variant after `Antigravity` in the enum
    - _Requirements: 6.1, 6.2_

- [x] 2. Implement Kiro install, uninstall, and show functions
  - [x] 2.1 Implement `run_kiro_mode` and `run_kiro_mode_at` in `src/hooks/init.rs`
    - Add `include_str!` constants for `KIRO_STEERING` and `KIRO_HOOK`
    - Define path constants: `KIRO_STEERING_DIR` (`.kiro/steering`), `KIRO_HOOKS_DIR` (`.kiro/hooks`), `KIRO_STEERING_FILE` (`rtk-rules.md`), `KIRO_HOOK_FILE` (`rtk-shell-rewrite.kiro.hook`)
    - Implement `run_kiro_mode` as a thin wrapper calling `run_kiro_mode_at` with `std::env::current_dir()`
    - Implement `run_kiro_mode_at(base_dir, ctx)`:
      1. Compute target paths relative to `base_dir`
      2. Create `.kiro/steering/` directory if missing (using `fs::create_dir_all`)
      3. Write steering file using `write_if_changed` (overwrite if exists, skip if identical)
      4. Create `.kiro/hooks/` directory if missing
      5. Write hook file using `write_if_changed`
      6. If hook write fails after steering write succeeded, remove the steering file (transactional rollback)
      7. Print success summary listing both relative paths
    - Support dry-run mode: print `[dry-run] would create directory` / `[dry-run] would write` messages
    - _Requirements: 1.1, 1.7, 1.8, 2.1, 2.7, 3.1, 3.3, 3.4, 5.1, 5.2, 5.3, 9.1, 9.2, 9.4, 9.5, 10.1, 10.2, 10.3, 10.4_

  - [x] 2.2 Implement `uninstall_kiro` and `uninstall_kiro_at` in `src/hooks/init.rs`
    - Implement `uninstall_kiro(ctx)` as a thin wrapper calling `uninstall_kiro_at` with `std::env::current_dir()`
    - Implement `uninstall_kiro_at(base_dir, ctx)`:
      1. Attempt to remove `.kiro/steering/rtk-rules.md` — record if removed
      2. Attempt to remove `.kiro/hooks/rtk-shell-rewrite.kiro.hook` — record if removed
      3. If file doesn't exist, skip silently (no error)
      4. Never remove directories (`.kiro/`, `.kiro/steering/`, `.kiro/hooks/`)
      5. Print summary of removed files, or "no RTK artifacts found" if nothing was removed
    - Support dry-run mode
    - _Requirements: 4.1, 4.2, 4.3, 4.4, 4.5, 4.6_

  - [x] 2.3 Implement `show_kiro_status` in `src/hooks/init.rs`
    - Check existence of both `.kiro/steering/rtk-rules.md` and `.kiro/hooks/rtk-shell-rewrite.kiro.hook`
    - Print `[ok] Kiro: steering + hook installed` when both present
    - Print `[warn] Kiro: steering installed, hook missing` or `[warn] Kiro: hook installed, steering missing` for partial
    - Print `[--] Kiro: not configured` when neither present
    - Integrate into the existing `show_config` function flow
    - _Requirements: 6.3_

- [x] 3. Wire CLI dispatch and global flag rejection
  - [x] 3.1 Add Kiro dispatch logic in `src/main.rs` Init command handler
    - Add `else if agent == Some(AgentTarget::Kiro)` branch after the Antigravity branch
    - Reject `--global` flag with `anyhow::bail!("Kiro is project-scoped. Use: rtk init --agent kiro")`
    - Route to `hooks::init::uninstall_kiro(ctx)` when `--uninstall` is passed
    - Route to `hooks::init::run_kiro_mode(ctx)` otherwise
    - _Requirements: 1.9, 3.2, 6.2, 6.4_

  - [x] 3.2 Wire `show_kiro_status` into the `show_config` / `--show` flow
    - Call `show_kiro_status()` from the existing `show_config` function so Kiro status appears alongside other agents
    - _Requirements: 6.3_

- [x] 4. Checkpoint - Ensure all tests pass
  - Ensure all tests pass, ask the user if questions arise.

- [ ] 5. Write unit tests for Kiro integration
  - [ ] 5.1 Write unit tests for install functionality
    - `test_kiro_mode_creates_steering_file`: verify file exists with correct content after `run_kiro_mode_at`
    - `test_kiro_mode_creates_hook_file`: verify hook file exists with valid JSON after `run_kiro_mode_at`
    - `test_kiro_hook_has_correct_schema`: parse hook JSON and assert `when.type == "preToolUse"`, `when.toolTypes == ["shell"]`, `then.type == "askAgent"`, `then.prompt` is non-empty
    - `test_kiro_steering_has_frontmatter`: assert content starts with `---\ninclusion: auto\n---`
    - `test_kiro_steering_has_examples`: assert at least 6 `rtk` command examples in the content
    - `test_kiro_mode_is_idempotent`: run twice, assert file contents are identical
    - `test_kiro_global_rejected`: verify `--global` with `--agent kiro` produces an error
    - _Requirements: 1.1, 1.2, 1.4, 1.8, 1.9, 2.1, 2.2, 2.3, 2.4, 2.5, 2.6, 3.2, 5.1, 5.2, 5.3_

  - [ ] 5.2 Write unit tests for uninstall functionality
    - `test_kiro_uninstall_removes_files`: install then uninstall, verify both files are gone
    - `test_kiro_uninstall_missing_files`: run uninstall when files don't exist, verify no error
    - `test_kiro_uninstall_preserves_other_files`: create extra files in `.kiro/steering/` and `.kiro/hooks/`, run uninstall, verify extra files remain
    - _Requirements: 4.1, 4.2, 4.3, 4.5, 4.6_

  - [ ] 5.3 Write unit tests for show status functionality
    - `test_kiro_show_status_both_present`: install both files, verify `[ok]` output
    - `test_kiro_show_status_partial`: install only steering file, verify `[warn]` output
    - `test_kiro_show_status_none`: verify `[--]` output when no files exist
    - `test_kiro_dry_run_no_writes`: run with `dry_run: true`, verify no files created
    - _Requirements: 6.3_

  - [ ]* 5.4 Write property test for installation creates both files (Property 1)
    - **Property 1: Installation creates both files at correct relative paths**
    - Generate random temp directory paths, run `run_kiro_mode_at`, assert both files exist with content matching `KIRO_STEERING` and `KIRO_HOOK` constants
    - Use `proptest` crate with minimum 100 iterations
    - **Validates: Requirements 1.1, 2.1, 3.1**

  - [ ]* 5.5 Write property test for idempotent installation (Property 2)
    - **Property 2: Installation is idempotent**
    - Generate random temp directories, run `run_kiro_mode_at` twice, assert file contents are byte-for-byte identical and second call returns Ok
    - Use `proptest` crate with minimum 100 iterations
    - **Validates: Requirements 1.8, 5.1, 5.2, 5.3**

  - [ ]* 5.6 Write property test for uninstall removes RTK files (Property 3)
    - **Property 3: Uninstall removes exactly the RTK files**
    - Generate random initial states (both files present, only one, neither), run `uninstall_kiro_at`, assert neither RTK file exists afterward and result is Ok
    - Use `proptest` crate with minimum 100 iterations
    - **Validates: Requirements 4.1, 4.2, 4.3**

  - [ ]* 5.7 Write property test for uninstall preserves non-RTK files (Property 4)
    - **Property 4: Uninstall preserves non-RTK directory contents**
    - Generate random additional files in `.kiro/steering/` and `.kiro/hooks/`, run `uninstall_kiro_at`, assert those files and directories still exist
    - Use `proptest` crate with minimum 100 iterations
    - **Validates: Requirements 4.6**

  - [ ]* 5.8 Write property test for transactional rollback (Property 5)
    - **Property 5: Transactional rollback on partial failure**
    - Use a test harness that injects write failure on the hook file (e.g., make `.kiro/hooks/` directory read-only), verify that after failure neither the steering file nor the hook file exists
    - Use `proptest` crate with minimum 100 iterations
    - **Validates: Requirements 10.3**

- [ ] 6. Checkpoint - Ensure all tests pass
  - Ensure all tests pass, ask the user if questions arise.

- [ ] 7. Update documentation
  - [ ] 7.1 Update `docs/guide/getting-started/supported-agents.md`
    - Add Kiro row to the supported agents table: `| Kiro IDE | Steering + Hook (project-scoped) | N/A |`
    - Add a "### Kiro IDE" installation section after Antigravity with command `rtk init --agent kiro`, describing both the steering file and hook file that are created
    - _Requirements: 8.1, 8.3, 8.4_

  - [ ] 7.2 Update `README.md` supported AI tools table
    - Add a row for Kiro with install command `rtk init --agent kiro` and method `.kiro/steering/rtk-rules.md (project-scoped)`
    - _Requirements: 8.2_

- [ ] 8. Final checkpoint - Ensure all tests pass
  - Ensure all tests pass, ask the user if questions arise.

## Notes

- Tasks marked with `*` are optional and can be skipped for faster MVP
- Each task references specific requirements for traceability
- Checkpoints ensure incremental validation
- Property tests validate universal correctness properties from the design document using the `proptest` crate
- Unit tests validate specific examples and edge cases
- The implementation follows existing patterns from `run_kilocode_mode` and `run_antigravity_mode` but adds transactional rollback semantics for the dual-file write
- All filesystem operations use `std::path::Path::join` for cross-platform compatibility (Requirement 9)

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1", "1.2", "1.3"] },
    { "id": 1, "tasks": ["2.1"] },
    { "id": 2, "tasks": ["2.2", "2.3", "3.1"] },
    { "id": 3, "tasks": ["3.2"] },
    { "id": 4, "tasks": ["5.1", "5.2", "5.3"] },
    { "id": 5, "tasks": ["5.4", "5.5", "5.6", "5.7", "5.8"] },
    { "id": 6, "tasks": ["7.1", "7.2"] }
  ]
}
```
