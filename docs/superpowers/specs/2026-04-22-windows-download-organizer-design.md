# Windows Download Organizer Design

Date: 2026-04-22
Status: Proposed
Owner: Codex

## Summary

Build a Windows tray application that runs in the background, watches the user's `Downloads` folder, and automatically classifies files into fixed subfolders based on file extension. Before moving each file, the app renames it to the format `dd-mm-yy_ten-tep.ext`, preserving the original extension and sanitizing the base name for Windows-safe paths.

The initial release targets a single personal workflow:

- Watch one folder: `Downloads`
- Move files into fixed subfolders inside `Downloads`
- Rename files using the current local date
- Resolve filename collisions safely
- Provide a small tray-based control surface rather than a full desktop shell

## Goals

- Remove manual sorting work from the `Downloads` folder
- Keep the app invisible during normal use and reachable from the system tray
- Process files shortly after downloads finish, without touching partial downloads
- Make behavior deterministic and easy to reason about
- Keep MVP implementation small enough to ship without a service architecture

## Non-Goals

- Classifying files by content, metadata, or AI inference
- Supporting multiple watched folders in MVP
- Detecting download source application in MVP
- Syncing files to cloud storage
- Supporting user-defined scripting or advanced rule expressions
- Running as a separate Windows service

## User Workflow

The user installs the app, enables "Start with Windows", and leaves it running in the tray.

When a new file appears in `Downloads`, the app waits until the file is stable, then:

1. Determines the destination subfolder from the file extension
2. Converts the filename into a sanitized slug-like base name
3. Prefixes the filename with the current date in `dd-mm-yy_` format
4. Moves the file into the destination subfolder under `Downloads`
5. Records the result in a recent activity log

Example:

- Original: `C:\Users\<user>\Downloads\Wedding Photo 01.PNG`
- Result: `C:\Users\<user>\Downloads\Images\22-04-26_wedding-photo-01.png`

## File Classification Rules

MVP ships with fixed extension groups:

- `PDF`: `.pdf`
- `Images`: `.jpg`, `.jpeg`, `.png`, `.gif`, `.webp`
- `Archives`: `.zip`, `.rar`, `.7z`
- `Docs`: `.doc`, `.docx`, `.xls`, `.xlsx`, `.ppt`, `.pptx`

Unmatched files remain in `Downloads` and are logged as skipped.

Extensions are matched case-insensitively. The app preserves the original extension semantics while normalizing the written extension to lowercase.

## Filename Normalization Rules

The app renames files to:

`dd-mm-yy_<sanitized-base-name>.<ext>`

Rules for `<sanitized-base-name>`:

- Strip the original extension before processing
- Convert to lowercase
- Replace whitespace runs with `-`
- Replace underscores with `-`
- Remove characters invalid on Windows paths
- Collapse repeated separators
- Trim leading and trailing separators
- Transliterate common accented Latin characters when possible
- Fall back to `file` if the sanitized name is empty

Collision handling:

- First choice: `22-04-26_ten-tep.pdf`
- If occupied: `22-04-26_ten-tep_02.pdf`
- Continue incrementing with zero-padded suffixes

## Desktop Experience

The app presents a tray icon with a small menu:

- `Open Settings`
- `View Recent Activity`
- `Run Scan Now`
- `Pause Automation` or `Resume Automation`
- `Start with Windows`
- `Open Downloads`
- `Quit`

No main dashboard is required for MVP. Settings and activity can open in lightweight windows.

## Architecture

The application is organized into four modules.

### 1. Tray UI

Responsibilities:

- Render the tray icon and menu
- Expose settings, activity, pause state, and manual scan
- Show short success or error notifications when appropriate

Technology direction:

- Tauri for desktop packaging and the lightweight webview shell
- Minimal frontend surface focused on settings and recent activity

### 2. Folder Watcher

Responsibilities:

- Observe the `Downloads` directory for file create or change events
- Ignore known temporary download artifacts such as `.crdownload`, `.tmp`, and `.part`
- Push candidate files into a processing queue instead of acting immediately

Key requirement:

- The watcher must avoid racing active browser downloads or other processes that still hold file handles.

### 3. Processing Engine

Responsibilities:

- Decide when a file is stable enough to process
- Map file extensions to categories
- Normalize and rename filenames
- Create destination folders on demand
- Move files safely
- Record success, skip, or failure results

Stability heuristic for MVP:

- Retry for a bounded period
- On each attempt, compare file size and modified time across a short interval
- Optionally attempt an open with the expected access mode
- Only process when the file stops changing

### 4. Config and Activity Store

Responsibilities:

- Persist folder mapping, naming preferences, startup preference, and pause state
- Persist recent activity entries for UI display

Storage choice for MVP:

- Local structured config file such as TOML or JSON
- Local activity log file or small embedded store

No full database is required unless activity history grows beyond simple local persistence needs.

## Data Model

Suggested config shape:

```toml
watch_folder = "C:\\Users\\RGB\\Downloads"
start_with_windows = true
notifications = true
paused = false
date_format = "dd-mm-yy"

[rules]
PDF = ["pdf"]
Images = ["jpg", "jpeg", "png", "gif", "webp"]
Archives = ["zip", "rar", "7z"]
Docs = ["doc", "docx", "xls", "xlsx", "ppt", "pptx"]
```

Suggested activity entry shape:

```json
{
  "timestamp": "2026-04-22T19:45:10+07:00",
  "original_path": "C:\\Users\\RGB\\Downloads\\Invoice FINAL.PDF",
  "result_path": "C:\\Users\\RGB\\Downloads\\PDF\\22-04-26_invoice-final.pdf",
  "status": "moved"
}
```

## Error Handling

Safety behavior is conservative:

- If a file is still locked after the retry budget, log an error and leave it untouched
- If rename or move fails, do not delete the source file
- If the destination folder is missing, create it and retry the move
- If classification does not match a rule, skip the file and log the outcome
- If the app restarts after downtime, `Run Scan Now` re-processes pending files in `Downloads`

The app should favor leaving a file in place over making a risky partial change.

## Testing Strategy

Unit tests:

- Extension-to-folder mapping
- Date prefix generation
- Filename sanitization
- Collision suffix generation
- Temporary file filtering

Integration tests:

- New completed file is moved and renamed correctly
- Partial download file is ignored until stable
- Existing destination filename triggers suffix increment
- Missing destination folder is created automatically
- Unknown extension remains in `Downloads`

Manual verification on Windows:

- Browser download completes into `Downloads`
- Tray controls work correctly after app startup
- "Start with Windows" persists and behaves as expected

## Technical Recommendation

Use Rust plus Tauri.

Reasons:

- Rust is a strong fit for filesystem watching, path handling, and low-overhead background work
- Tauri keeps packaging and UI lighter than Electron
- The required UI is modest, which makes Tauri a good fit for a tray-first desktop utility

## MVP Boundaries

The first shipped version is successful if it:

- Starts with Windows when enabled
- Watches `Downloads`
- Ignores temporary download files
- Renames files into `dd-mm-yy_ten-tep.ext`
- Moves files into fixed subfolders under `Downloads`
- Handles duplicate names without data loss
- Lets the user pause, inspect recent activity, and trigger a manual scan

Anything beyond those behaviors should be deferred until real usage reveals the next bottleneck.

## Implementation Notes for the Next Planning Step

The implementation plan should break work into:

- Tauri shell and tray scaffolding
- Config persistence
- Filesystem watching and stability detection
- Rename and move engine
- Activity log UI
- Windows startup integration
- Test coverage for naming and processing logic
