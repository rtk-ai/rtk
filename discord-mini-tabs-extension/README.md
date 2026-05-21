# Discord Mini Tabs

A Chrome Manifest V3 extension that opens saved Discord text and voice channel URLs in one reusable Chrome popup window.

## Development

Run pure logic tests:

```bash
npm test
```

Load in Chrome:

1. Open `chrome://extensions`.
2. Enable Developer mode.
3. Click Load unpacked.
4. Select the `discord-mini-tabs-extension` directory.

## Scope

Discord runs as the official Discord web app in a real Chrome popup window. This extension does not embed Discord, inject content scripts, read Discord messages, or inspect Discord voice internals.
