import { DEFAULT_BOUNDS, DEFAULT_ZOOM, SHORTCUT_TYPES } from "../shared/constants.js";
import { filterShortcuts, splitShortcutsByType } from "../shared/shortcuts.js";

export function formatWindowStatus(windowState) {
  return Number.isInteger(windowState?.windowId) ? "Open" : "Closed";
}

export function formatZoomPercent(zoom) {
  return `${Math.round(Number(zoom) * 100)}%`;
}

export function buildPopupModel({ shortcuts, query, activeType, windowState }) {
  const filteredShortcuts = filterShortcuts(shortcuts, query);
  const groupedShortcuts = splitShortcutsByType(filteredShortcuts);
  const normalizedActiveType =
    activeType === SHORTCUT_TYPES.VOICE ? SHORTCUT_TYPES.VOICE : SHORTCUT_TYPES.TEXT;
  const bounds = windowState?.bounds ?? {};
  const width = bounds.width ?? DEFAULT_BOUNDS.width;
  const height = bounds.height ?? DEFAULT_BOUNDS.height;

  return {
    status: formatWindowStatus(windowState),
    zoomLabel: formatZoomPercent(windowState?.zoom ?? DEFAULT_ZOOM),
    boundsLabel: `${width} x ${height}`,
    activeType: normalizedActiveType,
    activeShortcuts: groupedShortcuts[normalizedActiveType],
    textCount: groupedShortcuts.text.length,
    voiceCount: groupedShortcuts.voice.length
  };
}
