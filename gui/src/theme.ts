// Which of the three themes the page is in: the one asked for, or the
// system's light or dark. Named on the root before anything paints, and
// again whenever the system changes its mind.

export type Theme = "warm-black" | "ink-black" | "warm-white";

const themes: Theme[] = ["warm-black", "ink-black", "warm-white"];

function asked(): Theme | null {
  const fromAddress = new URLSearchParams(location.search).get("theme");
  if (themes.includes(fromAddress as Theme)) return fromAddress as Theme;
  try {
    const stored = localStorage.getItem("theme");
    if (themes.includes(stored as Theme)) return stored as Theme;
  } catch {
    // Storage may be closed to this page; the system's preference stands.
  }
  return null;
}

const light = matchMedia("(prefers-color-scheme: light)");

export function currentTheme(): Theme {
  return asked() ?? (light.matches ? "warm-white" : "warm-black");
}

/** Names the theme on the root, and calls back whenever it changes. */
export function applyTheme(changed: () => void): void {
  const apply = () => {
    const theme = currentTheme();
    if (document.documentElement.dataset.theme === theme) return;
    document.documentElement.dataset.theme = theme;
    changed();
  };
  document.documentElement.dataset.theme = currentTheme();
  light.addEventListener("change", apply);
}
