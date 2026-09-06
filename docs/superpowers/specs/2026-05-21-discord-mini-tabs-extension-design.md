# Discord Mini Tabs Chrome Extension Design

Date: 2026-05-21
Status: Approved for implementation planning

## Summary

Build a Chrome Manifest V3 extension that opens Discord web channels in a small, reusable Chrome popup window. The extension popup acts as a lightweight control panel for saved Discord text and voice channel shortcuts. Discord itself runs as the official Discord web app in a real Chrome window, so chat, voice UI, login, audio, and permissions remain handled by Discord and Chrome.

The default mini window size is 420x900. Users can change width, height, zoom, and window position. The extension remembers manual resize and move changes for future opens.

## Goals

- Open Discord text and voice channel URLs in one dedicated Chrome popup window.
- Let users chat and interact normally inside Discord web.
- Let users save shortcuts manually or from the current active Discord tab.
- Group shortcuts by Text and Voice, with search and compact window controls.
- Keep performance stable by avoiding embedded Discord, content-script DOM scraping, and multiple Discord instances.
- Store all shortcuts and settings locally with `chrome.storage.local`.

## Non-Goals

- Do not clone or reimplement Discord UI.
- Do not embed Discord in an iframe or extension page.
- Do not auto-click Join Voice, mute, deafen, or other Discord controls.
- Do not read Discord messages, inspect Discord DOM, or scrape internal voice state.
- Do not provide real bitrate telemetry. Any voice/audio status shown by the popup is limited to Chrome-level window or tab state if available.
- Do not sync settings across devices in the first version.

## Architecture

The extension has three main parts:

- `manifest.json`: Manifest V3 metadata, permissions, host permissions, popup entry, and service worker.
- Popup UI: user-facing control panel for shortcuts, search, window controls, and settings.
- Background service worker: owns Chrome API interactions for windows, tabs, zoom, and storage.

The background service worker stores and manages:

- `windowId`: the current Discord mini popup window id, if it still exists.
- `tabId`: the Discord tab inside the popup window, if it still exists.
- saved shortcuts.
- mini window bounds: `left`, `top`, `width`, `height`.
- default and current zoom.
- last opened shortcut id.

When the user opens a shortcut, the service worker validates the saved URL, checks whether the stored `windowId` and `tabId` are still valid, and then either:

- creates a new Chrome popup window with the Discord URL, or
- focuses the existing popup window and updates its existing tab URL.

The extension should use one Discord mini window at a time. This keeps memory, CPU, and WebRTC/audio surface lower than opening one Discord instance per channel.

## Permissions

Use the minimum permissions needed for the agreed behavior:

- `storage`: save shortcuts, settings, bounds, zoom, and last opened shortcut.
- `tabs`: read/update tab state and set zoom.
- `windows`: create, focus, close, and track the mini popup window.
- `activeTab`: support saving the current Discord channel from the active tab.
- host permissions for `https://discord.com/*`.

The initial version targets stable Discord web URLs:

```text
https://discord.com/channels/{serverId}/{channelId}
https://discord.com/channels/@me/{channelId}
```

`https://canary.discord.com/*` and `https://ptb.discord.com/*` can be added later if needed, but are not required for the first version.

## Popup UI

The popup is a compact command surface, not a Discord renderer.

Primary sections:

- Header with extension name and mini window status: `Closed`, `Open`, or `Focused` when known.
- Search input for shortcut name, server label, channel label, or URL.
- Segmented control for `Text` and `Voice`.
- Shortcut list grouped by type.
- Mini window controls: `Focus`, `Close`, `Reset position`.
- Settings controls: `Width`, `Height`, `Zoom`, and `Apply`.
- Add shortcut controls:
  - manual URL input and display name.
  - `Save current Discord channel` action.

Each shortcut item should show:

- display name.
- type: `text` or `voice`.
- a shortened URL or channel identifier.
- actions: `Open`, `Edit`, `Delete`.

The popup should remain fast and simple. It should not load remote assets, render Discord previews, or maintain a long-running connection.

## Data Model

Storage key shape:

```json
{
  "shortcuts": [
    {
      "id": "uuid-or-stable-random-id",
      "name": "dev-chat",
      "type": "text",
      "url": "https://discord.com/channels/123456789012345678/987654321098765432",
      "createdAt": "2026-05-21T00:00:00.000Z",
      "updatedAt": "2026-05-21T00:00:00.000Z"
    }
  ],
  "windowState": {
    "windowId": 123,
    "tabId": 456,
    "bounds": {
      "left": 1200,
      "top": 80,
      "width": 420,
      "height": 900
    },
    "zoom": 0.9,
    "lastShortcutId": "uuid-or-stable-random-id"
  }
}
```

The defaults are:

- width: `420`
- height: `900`
- zoom: `0.9`
- position: near the right edge of the primary display when Chrome allows it.

Bounds and zoom should be clamped to reasonable values before saving or applying:

- minimum width: `320`
- minimum height: `480`
- maximum width: `1200`
- maximum height: `1400`
- zoom range: `0.67` to `1.25`

## User Flows

### Open Shortcut

1. User clicks `Open` on a saved Text or Voice shortcut.
2. Popup sends `OPEN_SHORTCUT` to the service worker.
3. Service worker validates the URL.
4. Service worker resolves existing mini window and tab state.
5. If the mini window is alive, focus it and update the tab URL.
6. If the mini window is missing or invalid, create a Chrome popup window using the saved bounds.
7. Apply saved zoom to the Discord tab.
8. Store current `windowId`, `tabId`, and `lastShortcutId`.

### Save Current Discord Channel

1. User opens a Discord channel in a normal Chrome tab.
2. User opens the extension popup and clicks `Save current Discord channel`.
3. Popup asks the service worker to inspect the active tab.
4. Service worker accepts only `discord.com/channels/...` URLs.
5. Popup proposes a display name from tab title when possible.
6. User chooses Text or Voice and saves.

The extension does not need to infer whether a URL is truly text or voice. The user selects the type, and both types are opened through the same Discord URL mechanism.

### Resize Or Move Mini Window

1. User manually resizes or moves the Chrome popup window.
2. Background receives `chrome.windows.onBoundsChanged`.
3. Background debounces storage writes.
4. New bounds are saved to `chrome.storage.local`.
5. Next open uses the remembered size and position.

### Update Size Or Zoom

1. User changes width, height, or zoom in popup settings.
2. Popup sends `UPDATE_WINDOW_SETTINGS`.
3. Service worker stores the settings.
4. If the mini window exists, service worker applies bounds with `chrome.windows.update`.
5. If the Discord tab exists, service worker applies zoom with `chrome.tabs.setZoom`.

## Error Handling

- If stored `windowId` no longer exists, clear it and create a new mini window on the next open.
- If stored `tabId` no longer exists, locate the Discord tab in the mini window or create/update a tab as needed.
- If the mini window is closed manually, clear window state when Chrome reports removal.
- If a saved URL is invalid, block open and show a short error in the popup.
- If `Save current Discord channel` is used outside Discord, show that the active tab is not a Discord channel.
- If Chrome rejects requested bounds because of display limits, fall back to default bounds and let Chrome choose a valid position.
- If zoom cannot be applied, keep the window usable and show a non-blocking popup error.

## Performance And Stability

The design favors stability over deep Discord integration:

- One Discord web instance at a time.
- No content script injection into Discord.
- No DOM polling.
- No background network requests.
- No audio/WebRTC handling by the extension.
- Debounced bounds persistence to avoid excessive storage writes.
- Small local data model stored in `chrome.storage.local`.

Discord web remains responsible for:

- login session.
- chat rendering and sending.
- voice channel UI.
- microphone/audio permissions.
- connection quality and reconnect behavior.

## Testing Plan

### Logic Tests

- Validate accepted Discord channel URLs.
- Reject malformed URLs and non-Discord hosts.
- Filter shortcuts by search query.
- Clamp width, height, and zoom settings.
- Recover from missing `windowId` or `tabId`.

### Manual Chrome Extension Tests

- Load the extension unpacked in Chrome.
- Add a shortcut manually.
- Save the current Discord channel from an active Discord tab.
- Open a text shortcut and send a test message in Discord web.
- Open a voice shortcut and verify Discord web handles voice UI.
- Switch between shortcuts and confirm only one mini window is reused.
- Resize and move the mini window, close it, then reopen and verify remembered bounds.
- Change zoom and verify it applies to the Discord tab.
- Use Focus, Close, and Reset position.
- Reload the extension and verify stored shortcuts/settings remain.

### Stability Smoke Test

- Open the mini window and leave it running for an extended session.
- Switch between several text and voice shortcuts repeatedly.
- Confirm the extension does not open duplicate Discord windows.
- Confirm closing the mini window and reopening recovers cleanly.
- Confirm invalid shortcuts do not crash the popup or service worker.

## Implementation Scope

The first implementation should create a standalone extension codebase in a dedicated directory rather than modifying the existing Rust CLI project. A suitable directory name is `discord-mini-tabs-extension`.

The implementation should be plain, dependency-light TypeScript or JavaScript unless a build step is intentionally chosen during implementation planning. Given the stability goal, the default recommendation is a small MV3 extension with minimal tooling and no runtime framework.

## Open Follow-Ups For Implementation Planning

- Choose plain JavaScript versus TypeScript with a small build step.
- Decide exact popup visual style and icon set.
- Decide whether to support `canary.discord.com` and `ptb.discord.com` in the first release.
- Decide whether to include import/export of shortcuts in the first release.
