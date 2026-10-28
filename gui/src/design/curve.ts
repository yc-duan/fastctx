// A curve of the design tokens (`tokens.css`), such as `--ease-out`, for an
// animation a script plays: read when it plays, so it is the theme's own.

export function curve(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || "ease";
}
