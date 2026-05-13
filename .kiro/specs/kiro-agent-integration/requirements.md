# Requirements Document

## Introduction

Add official Kiro IDE support to RTK (Rust Token Killer) as a rules-file integration. The `rtk init --agent kiro` command will set up RTK integration for the Kiro IDE by creating a steering file (`.kiro/steering/rtk-rules.md`) with auto-inclusion frontmatter and a preToolUse hook (`.kiro/hooks/rtk-shell-rewrite.kiro.hook`) as an additional reminder. This follows the same pattern as existing rules-file integrations for Windsurf, Cline, Kilo Code, and Antigravity.

## Glossary

- **RTK**: Rust Token Killer — a CLI proxy that compresses command outputs to save LLM tokens (60-90% savings)
- **Init_Command**: The `rtk init` CLI command responsible for installing agent integrations
- **Kiro_Integration**: The set of files and logic that enables RTK usage within the Kiro IDE
- **Steering_File**: A markdown file in `.kiro/steering/` with YAML frontmatter (`inclusion: auto`) that Kiro always loads into agent context
- **Hook_File**: A JSON file in `.kiro/hooks/` that defines event-driven agent behavior in Kiro IDE
- **PreToolUse_Hook**: A Kiro hook triggered before tool execution, using `askAgent` action to prompt the agent
- **Rules_File_Integration**: An RTK integration tier that relies on prompt-level instructions rather than transparent command rewriting
- **Agent_Target**: The enum in RTK source code representing supported agent identifiers for the `--agent` flag
- **Uninstall_Mode**: The `--uninstall` flag that removes previously installed integration files

## Requirements

### Requirement 1: Install Kiro Steering File

**User Story:** As a developer using Kiro IDE, I want to run `rtk init --agent kiro` so that RTK steering instructions are automatically installed in my project.

#### Acceptance Criteria

1. WHEN the user runs `rtk init --agent kiro`, THE Init_Command SHALL create a file at `.kiro/steering/rtk-rules.md` in the current working directory
2. THE Steering_File SHALL contain YAML frontmatter with `inclusion: auto` as the first line of the file, enclosed in `---` delimiters
3. THE Steering_File SHALL contain instructions directing the Kiro agent to prefix shell commands with `rtk`
4. THE Steering_File SHALL include at least 6 examples of RTK-prefixed commands covering file operations (ls, read, find, grep), version control (git), and build tools (cargo, docker)
5. THE Steering_File SHALL include a section listing RTK meta commands (gain, discover, proxy)
6. THE Steering_File SHALL include guidance on when NOT to use the rtk prefix (interactive commands, binary output, already-prefixed commands)
7. WHEN the `.kiro/steering/` directory does not exist, THE Init_Command SHALL create the directory before writing the file
8. IF the file `.kiro/steering/rtk-rules.md` already exists with identical content, THEN THE Init_Command SHALL skip writing and indicate the file is up to date
9. IF the user passes the `--global` flag with `--agent kiro`, THEN THE Init_Command SHALL reject the command with an error message indicating that Kiro steering is project-scoped

### Requirement 2: Install Kiro PreToolUse Hook

**User Story:** As a developer using Kiro IDE, I want the RTK init command to install a preToolUse hook so that the Kiro agent receives an additional reminder to use RTK before executing shell commands.

#### Acceptance Criteria

1. WHEN the user runs `rtk init --agent kiro`, THE Init_Command SHALL create a file at `.kiro/hooks/rtk-shell-rewrite.kiro.hook` in the current working directory
2. THE Hook_File SHALL be valid JSON conforming to the Kiro hook schema with the fields: `name` (string), `version` (string), `description` (string), `when` (object), and `then` (object)
3. THE Hook_File SHALL specify `preToolUse` as the event type in the `when.type` field
4. THE Hook_File SHALL specify `["shell"]` in the `when.toolTypes` array
5. THE Hook_File SHALL specify `askAgent` as the action type in the `then.type` field
6. THE Hook_File SHALL contain a prompt in `then.prompt` that instructs the agent to check whether the command is prefixed with `rtk`, rewrite supported commands to use the `rtk` prefix, and not add the prefix to interactive commands, already-prefixed commands, or unsupported commands
7. WHEN the `.kiro/hooks/` directory does not exist, THE Init_Command SHALL create the directory before writing the file

### Requirement 3: Project-Scoped Installation

**User Story:** As a developer, I want the Kiro integration to be project-scoped so that each project can independently opt into RTK integration.

#### Acceptance Criteria

1. THE Init_Command SHALL install Kiro integration files relative to the current working directory
2. WHEN the user passes the `--global` flag with `--agent kiro`, THE Init_Command SHALL reject the command with a non-zero exit code and an error message indicating that Kiro integration is project-scoped
3. WHEN installation completes successfully, THE Init_Command SHALL display a success message listing the relative paths of the files created (`.kiro/steering/rtk-rules.md` and `.kiro/hooks/rtk-shell-rewrite.kiro.hook`)
4. WHEN installation completes successfully, THE Init_Command SHALL return exit code 0

### Requirement 4: Uninstall Kiro Integration

**User Story:** As a developer, I want to run `rtk init --agent kiro --uninstall` so that previously installed Kiro integration files are cleanly removed.

#### Acceptance Criteria

1. WHEN the user runs `rtk init --agent kiro --uninstall`, THE Init_Command SHALL remove the file at `.kiro/steering/rtk-rules.md`
2. WHEN the user runs `rtk init --agent kiro --uninstall`, THE Init_Command SHALL remove the file at `.kiro/hooks/rtk-shell-rewrite.kiro.hook`
3. IF a target file does not exist during uninstall, THEN THE Init_Command SHALL skip that file without returning an error
4. WHEN the uninstall operation completes and at least one file was removed, THE Init_Command SHALL display a summary listing each removed file path
5. IF no target files existed during uninstall (all files were already absent), THEN THE Init_Command SHALL display a message indicating that no RTK artifacts were found to remove
6. THE Init_Command SHALL NOT remove the `.kiro/steering/`, `.kiro/hooks/`, or `.kiro/specs/` directories themselves (other files may exist in them)

### Requirement 5: Idempotent Installation

**User Story:** As a developer, I want running `rtk init --agent kiro` multiple times to be safe so that I do not end up with corrupted or duplicated configuration.

#### Acceptance Criteria

1. WHEN the user runs `rtk init --agent kiro` and the steering file already exists, THE Init_Command SHALL overwrite the file with the current template content
2. WHEN the user runs `rtk init --agent kiro` and the hook file already exists, THE Init_Command SHALL overwrite the file with the current template content
3. FOR ALL valid project directories, running the install command twice SHALL produce the same file contents as running it once (idempotence property)

### Requirement 6: Agent Target Registration

**User Story:** As a developer, I want `kiro` to be a recognized value for the `--agent` flag so that the CLI validates and routes the command correctly.

#### Acceptance Criteria

1. THE Agent_Target enum SHALL include a `Kiro` variant
2. WHEN the user passes `--agent kiro` (case-insensitive), THE Init_Command SHALL accept the value without returning a validation error and SHALL proceed to execute Kiro-specific initialization logic
3. WHEN the user runs `rtk init --show`, THE Init_Command SHALL display a Kiro integration status line prefixed with `[ok]`, `[warn]`, or `[--]` indicating whether Kiro hooks are installed, partially configured, or not found
4. IF the user passes `--agent kiro` with the `--global` flag and Kiro integration is project-scoped, THEN THE Init_Command SHALL reject the command with an error message indicating that Kiro is project-scoped

### Requirement 7: Steering File Content Template

**User Story:** As a developer, I want the installed steering file to follow the same content pattern as other rules-file integrations so that RTK behavior is consistent across agents.

#### Acceptance Criteria

1. THE Steering_File template SHALL be stored as a markdown file named `rules.md` in the `hooks/kiro/` directory of the RTK source tree
2. THE Steering_File template SHALL contain the following sections in order: a level-1 heading title, a bold "Usage" summary line, a "Rule" section, an "Examples" subsection with fenced code blocks, a "Meta Commands" section with fenced code blocks, and a "Why" section — matching the section structure of `hooks/windsurf/rules.md`
3. THE Steering_File template SHALL include the YAML frontmatter block (`---\ninclusion: auto\n---`) as the first content before the level-1 heading
4. THE Steering_File template SHALL include the text "Kiro" in the level-1 heading of the document

### Requirement 8: Documentation Update

**User Story:** As a developer evaluating RTK, I want to see Kiro listed in the supported agents documentation so that I know RTK works with my IDE.

#### Acceptance Criteria

1. THE supported agents table in `docs/guide/getting-started/supported-agents.md` SHALL include a row for Kiro with install command `rtk init --agent kiro` and method `.kiro/steering + .kiro/hooks (project-scoped)`
2. THE README.md supported AI tools table SHALL include a row for Kiro with install command `rtk init --agent kiro` and method `.kiro/steering/rtk-rules.md (project-scoped)`
3. THE supported agents documentation SHALL include an installation section for Kiro showing the command `rtk init --agent kiro`
4. THE installation section SHALL describe both the steering file and the hook file that are created

### Requirement 9: Windows Compatibility

**User Story:** As a Windows developer, I want the Kiro integration to work natively on Windows without requiring WSL so that I can use RTK with Kiro on my native environment.

#### Acceptance Criteria

1. WHEN the Init_Command creates Kiro integration files on Windows, THE Init_Command SHALL use backslash path separators for filesystem operations and forward slashes when displaying relative paths in console output
2. THE Init_Command SHALL create directories and files using platform-native filesystem APIs (e.g., `std::path::Path::join`, `std::fs::create_dir_all`) that resolve OS-appropriate separators without requiring a Unix shell or Unix-specific utilities
3. THE Steering_File content SHALL use forward slashes in all example paths and shell command snippets so that examples remain valid across Windows (cmd, PowerShell) and Unix shells
4. WHEN the Init_Command runs on Windows, THE Init_Command SHALL resolve the current working directory using the native Windows path format (e.g., `C:\Users\dev\project`) without requiring path translation from Unix-style paths
5. IF the target directory path contains spaces or special characters valid on Windows, THEN THE Init_Command SHALL create files successfully without path-handling errors

### Requirement 10: Graceful Error Handling

**User Story:** As a developer, I want clear error messages when something goes wrong during Kiro integration setup so that I can resolve issues quickly.

#### Acceptance Criteria

1. IF the current directory is not writable, THEN THE Init_Command SHALL return a non-zero exit code and an error message that includes the directory path and indicates insufficient write permissions
2. IF file creation fails for any reason, THEN THE Init_Command SHALL return a non-zero exit code and an error message that includes the file path that failed and the reason for failure
3. IF an error occurs after one or more files have been created during the current init operation, THEN THE Init_Command SHALL remove all files created during that operation before exiting, leaving the directory in the same state as before the command was invoked
4. IF cleanup of partially-created files itself fails, THEN THE Init_Command SHALL return a non-zero exit code and an error message listing the files that could not be removed
