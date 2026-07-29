// The "open files with" setting: where a file provenance pointer opens on
// drill-down. "system" = the OS default app; the rest are editors the Rust
// side opens via their URL scheme (VS Code family, Zed) or CLI (Xcode),
// jumping to the evidence line when the pointer carries one.
export type Editor =
  | "system"
  | "vscode"
  | "cursor"
  | "zed"
  | "windsurf"
  | "xcode";

// Proper nouns, not translated; "system" gets its label from i18n.
export const EDITOR_NAMES: [Editor, string][] = [
  ["vscode", "Visual Studio Code"],
  ["cursor", "Cursor"],
  ["zed", "Zed"],
  ["windsurf", "Windsurf"],
  ["xcode", "Xcode"],
];

const LS_KEY = "witnos.editor";

export function detectEditor(): Editor {
  const saved = localStorage.getItem(LS_KEY);
  return saved && (saved === "system" || EDITOR_NAMES.some(([v]) => v === saved))
    ? (saved as Editor)
    : "system";
}

export function saveEditor(e: Editor) {
  localStorage.setItem(LS_KEY, e);
}
