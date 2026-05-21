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

function normalizeObject(value) {
  return value && typeof value === "object" ? value : {};
}

export function clampBounds(bounds = {}) {
  const source = normalizeObject(bounds);

  return {
    left: normalizeNullableCoordinate(source.left),
    top: normalizeNullableCoordinate(source.top),
    width: clampNumber(
      source.width,
      BOUNDS_LIMITS.minWidth,
      BOUNDS_LIMITS.maxWidth,
      DEFAULT_BOUNDS.width
    ),
    height: clampNumber(
      source.height,
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
  const source = normalizeObject(input);
  const bounds = normalizeObject(source.bounds);

  return {
    windowId: Number.isInteger(source.windowId) ? source.windowId : null,
    tabId: Number.isInteger(source.tabId) ? source.tabId : null,
    bounds: clampBounds({ ...defaults.bounds, ...bounds }),
    zoom: clampZoom(source.zoom ?? defaults.zoom),
    lastShortcutId:
      typeof source.lastShortcutId === "string" && source.lastShortcutId.length > 0
        ? source.lastShortcutId
        : null
  };
}
