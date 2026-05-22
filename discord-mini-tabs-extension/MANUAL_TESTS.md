# Manual Tests

## Load Unpacked

1. Open `chrome://extensions`.
2. Enable Developer mode.
3. Load the `C:\Users\RGB\rtk\.worktrees\discord-mini-tabs-extension\discord-mini-tabs-extension` directory.
4. Confirm Chrome shows no manifest or service worker registration errors.

## Shortcut Management

1. Add a text shortcut with `https://discord.com/channels/123456789012345678/987654321098765432`.
2. Confirm the shortcut appears in the Text list.
3. Edit the shortcut name.
4. Search for the new name.
5. Delete the shortcut.
6. Confirm the list updates without reopening the popup.

## Save Current Discord Channel

1. Open a real Discord channel in a normal Chrome tab.
2. Open the extension popup.
3. Click Save current.
4. Confirm the URL and suggested name populate the form.
5. Save the shortcut.

## Mini Window

1. Open a saved text shortcut.
2. Confirm one Chrome popup window opens.
3. Send a Discord chat message in that window.
4. Open another shortcut.
5. Confirm the same popup window is reused.
6. Resize the popup window.
7. Close and reopen a shortcut.
8. Confirm the remembered size is used.

## Voice URL

1. Save a Discord voice channel URL as type Voice.
2. Open it from the Voice list.
3. Confirm Discord web displays the voice channel UI.
4. Join or leave voice manually through Discord web.

## Window Controls

1. Click Focus and confirm the mini window comes forward.
2. Click Close and confirm the mini window closes.
3. Click Reset position and confirm the next open lets Chrome choose a valid position with the saved size.

## Settings

1. Set width to `420`, height to `900`, and zoom to `90`.
2. Open a shortcut.
3. Confirm the mini window size and Discord tab zoom are applied.
4. Change width to `500` and zoom to `100`.
5. Confirm the existing mini window updates.

## Stability Smoke Test

1. Switch between several text and voice shortcuts repeatedly.
2. Confirm only one Discord mini window remains open.
3. Close the mini window manually.
4. Open a shortcut again.
5. Confirm the extension recovers without errors.

## Service Worker Errors

1. Open `chrome://extensions`.
2. Open the service worker inspection link for Discord Mini Tabs.
3. Confirm there are no uncaught exceptions after opening the popup, adding a shortcut, opening a shortcut, focusing, closing, and changing settings.
