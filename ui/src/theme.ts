// Appearance. The app ships a light and a dark palette (the two token blocks in
// index.css); which one paints is the `data-theme` attribute on <html>. This
// module is the only thing that writes it.
//
// "system" follows the OS and keeps following it while the app is open — a
// desktop that flips at sunset takes the app with it. "light"/"dark" pin it.
import { getCurrentWindow } from "@tauri-apps/api/window";

export type Theme = "system" | "light" | "dark";
/** What actually paints — "system" is always resolved before it gets here. */
export type Appearance = "light" | "dark";

const LS_KEY = "witnos.theme";
const LIGHT_MQ = "(prefers-color-scheme: light)";

export function detectTheme(): Theme {
  const saved = localStorage.getItem(LS_KEY);
  return saved === "light" || saved === "dark" ? saved : "system";
}

export function saveTheme(t: Theme) {
  localStorage.setItem(LS_KEY, t);
}

export function resolveAppearance(t: Theme): Appearance {
  if (t !== "system") return t;
  return window.matchMedia(LIGHT_MQ).matches ? "light" : "dark";
}

/** Repaints the UI. Called once before the first render too, so the window can
 *  never flash the wrong palette on a light desktop. */
export function applyAppearance(a: Appearance) {
  document.documentElement.dataset.theme = a;
}

/** The window frame is the OS's, not the webview's — the traffic lights only
 *  follow a pinned theme if we say so, and null hands the window back to the
 *  system. Best-effort: the CSS side has already repainted, so a failure here
 *  costs the frame's appearance, nothing more. */
export function syncWindowTheme(t: Theme) {
  try {
    getCurrentWindow()
      .setTheme(t === "system" ? null : t)
      .catch(() => {});
  } catch {
    // Not running inside Tauri (e.g. the frontend opened in a plain browser).
  }
}

/** Fires when the OS appearance flips. Only worth subscribing to while the
 *  preference is "system". */
export function watchSystemAppearance(
  onChange: (a: Appearance) => void,
): () => void {
  const mq = window.matchMedia(LIGHT_MQ);
  const handler = (e: MediaQueryListEvent) =>
    onChange(e.matches ? "light" : "dark");
  mq.addEventListener("change", handler);
  return () => mq.removeEventListener("change", handler);
}
