import {
  BOUNDS_LIMITS,
  DEFAULT_BOUNDS,
  DEFAULT_ZOOM,
  MAX_ZOOM,
  MIN_ZOOM
} from "./constants.js";

function clampNumber(value, min, max, fallback) {
  const number = Number(value);
  if (!Number.isFinite(number)) {
    return fallback;
  }
  return Math.min(max, Math.max(min, Math.round(number)));
}

function normalizeNullableCoordinate(value) {
  if (value === null || value === undefined) {
    return null;
  }

  const number = Number(value);
  return Number.isFinite(number) ? Math.round(number) : null;
}

export function clampBounds(bounds = {}) {
  return {
    left: normalizeNullableCoordinate(bounds.left),
    top: normalizeNullableCoordinate(bounds.top),
    width: clampNumber(
      bounds.width,
      BOUNDS_LIMITS.minWidth,
      BOUNDS_LIMITS.maxWidth,
      DEFAULT_BOUNDS.width
    ),
    height: clampNumber(
      bounds.height,
      BOUNDS_LIMITS.minHeight,
      BOUNDS_LIMITS.maxHeight,
      DEFAULT_BOUNDS.height
    )
  };
}

export function clampZoom(value) {
  const number = Number(value);
  if (!Number.isFinite(number)) {
    return DEFAULT_ZOOM;
  }
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, Number(number.toFixed(2))));
}

export function createDefaultWindowState() {
  return {
    windowId: null,
    tabId: null,
    bounds: { ...DEFAULT_BOUNDS },
    zoom: DEFAULT_ZOOM,
    lastShortcutId: null
  };
}

export function normalizeWindowState(input = {}) {
  const defaults = createDefaultWindowState();
  return {
    windowId: Number.isInteger(input.windowId) ? input.windowId : null,
    tabId: Number.isInteger(input.tabId) ? input.tabId : null,
    bounds: clampBounds({ ...defaults.bounds, ...input.bounds }),
    zoom: clampZoom(input.zoom ?? defaults.zoom),
    lastShortcutId:
      typeof input.lastShortcutId === "string" && input.lastShortcutId.length > 0
        ? input.lastShortcutId
        : null
  };
}
