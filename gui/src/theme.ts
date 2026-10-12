// Which of the three themes the page is in: the one chosen, or the
// system's light or dark. Named on the root before anything paints
// (`index.html`), and again whenever the choice or the system changes. The
// text size is kept the same way. A phone's shell is told each time, for the
// window behind the page and its bars (`system-bars.ts`).

import { paintBars } from "./system-bars";

export type Theme = "warm-black" | "ink-black" | "warm-white";

export const themes: Theme[] = ["warm-black", "ink-black", "warm-white"];

function stored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    // Storage may be closed to this page; the defaults stand.
    return null;
  }
}

function store(key: string, value: string | null): void {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    // Not kept: the choice holds until the window closes.
  }
}

/** The theme chosen, or `null` to follow the system. */
export function chosenTheme(): Theme | null {
  const fromAddress = new URLSearchParams(location.search).get("theme");
  if (themes.includes(fromAddress as Theme)) return fromAddress as Theme;
  const kept = stored("theme");
  return themes.includes(kept as Theme) ? (kept as Theme) : null;
}

const light = matchMedia("(prefers-color-scheme: light)");

export function currentTheme(): Theme {
  return chosenTheme() ?? (light.matches ? "warm-white" : "warm-black");
}

let changed: () => void = () => {};

function apply(): void {
  const theme = currentTheme();
  paintBars(theme, chosenTheme() !== null);
  if (document.documentElement.dataset.theme === theme) return;
  document.documentElement.dataset.theme = theme;
  changed();
}

/** Names the theme on the root, and calls back whenever it changes. */
export function applyTheme(onChange: () => void): void {
  changed = onChange;
  document.documentElement.dataset.theme = currentTheme();
  paintBars(currentTheme(), chosenTheme() !== null);
  light.addEventListener("change", apply);
}

export function chooseTheme(theme: Theme | null): void {
  store("theme", theme);
  apply();
}

export const textSizes = ["14", "15", "16", "17"];

/** The body size chosen, or `null` for the screen's own. */
export function chosenSize(): string | null {
  const kept = stored("bodySize");
  return kept && textSizes.includes(kept) ? kept : null;
}

export function chooseSize(size: string | null): void {
  store("bodySize", size);
  if (size) document.documentElement.style.setProperty("--body-size", `${size}px`);
  else document.documentElement.style.removeProperty("--body-size");
}
