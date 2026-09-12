export const STORAGE_KEYS = {
  SHORTCUTS: "shortcuts",
  WINDOW_STATE: "windowState"
};

export const SHORTCUT_TYPES = {
  TEXT: "text",
  VOICE: "voice"
};

export const DEFAULT_BOUNDS = {
  left: null,
  top: null,
  width: 420,
  height: 900
};

export const BOUNDS_LIMITS = {
  minWidth: 320,
  minHeight: 480,
  maxWidth: 1200,
  maxHeight: 1400
};

export const DEFAULT_ZOOM = 0.9;
export const MIN_ZOOM = 0.67;
export const MAX_ZOOM = 1.25;

export const DISCORD_HOST = "discord.com";
export const DISCORD_CHANNEL_PREFIX = "/channels/";

export const MESSAGE_TYPES = {
  GET_STATE: "GET_STATE",
  CREATE_SHORTCUT: "CREATE_SHORTCUT",
  UPDATE_SHORTCUT: "UPDATE_SHORTCUT",
  DELETE_SHORTCUT: "DELETE_SHORTCUT",
  OPEN_SHORTCUT: "OPEN_SHORTCUT",
  READ_ACTIVE_DISCORD_TAB: "READ_ACTIVE_DISCORD_TAB",
  UPDATE_WINDOW_SETTINGS: "UPDATE_WINDOW_SETTINGS",
  FOCUS_WINDOW: "FOCUS_WINDOW",
  CLOSE_WINDOW: "CLOSE_WINDOW",
  RESET_POSITION: "RESET_POSITION"
};
