// ⌘B on macOS, Ctrl+B elsewhere — the toggle lives in App's keydown handler
const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

export const toggleRailShortcut = isMac ? "⌘B" : "Ctrl+B";
