// The World's styles as a page wears them. A page names its theme on its
// root as `data-world-theme`, not `data-theme`: a page written with the
// official MCP Apps SDK writes "light" or "dark" into `data-theme` itself,
// and the two would undo each other. Styles that reach into an element's
// shadow root name the theme through the host's ancestors.

/** For the page's document. */
export function themed(css: string): string {
  return css.replace(/\[data-theme=/g, "[data-world-theme=");
}

/** For a style sheet inside an element's shadow root. */
export function themedWithin(css: string): string {
  return css.replace(/\[data-theme=("[^"]+")\]/g, ":host-context([data-world-theme=$1])");
}
