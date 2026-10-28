// Diagrams are drawn once the words around them are done: a half-written
// diagram is not one. Mermaid loads the first time a diagram needs it,
// takes its colours from the page's tokens, and runs strict, so a diagram
// can never bring script or HTML of its own. One it cannot draw keeps its
// source in view.

type Mermaid = typeof import("mermaid").default;

let loading: Promise<Mermaid> | null = null;
let theme = "";
let serial = 0;
/** Drawings by source, theme and text size, so a diagram scrolled out and
 *  back, or drawn again in another view, is not laid out anew. */
const drawings = new Map<string, string>();
const KEPT = 48;

export async function drawDiagrams(root: HTMLElement): Promise<void> {
  const waiting = [...root.querySelectorAll<HTMLElement>("figure.diagram[data-mermaid]:not(.drawn):not(.unreadable)")].filter(
    (figure) => !figure.closest(".writing"),
  );
  if (!waiting.length) return;
  const mermaid = await load();
  const size = getComputedStyle(document.documentElement).getPropertyValue("--body-size").trim();
  for (const figure of waiting) {
    if (figure.classList.contains("drawn") || !figure.isConnected) continue;
    const source = figure.textContent ?? "";
    const key = `${theme}\n${size}\n${source}`;
    let svg = drawings.get(key);
    if (svg === undefined) {
      serial += 1;
      try {
        svg = (await mermaid.render(`diagram-${serial}`, source)).svg;
      } catch {
        figure.classList.add("unreadable");
        continue;
      }
      drawings.set(key, svg);
      for (const oldest of drawings.keys()) {
        if (drawings.size <= KEPT) break;
        drawings.delete(oldest);
      }
    }
    if (!figure.isConnected || figure.classList.contains("drawn")) continue;
    figure.innerHTML = svg;
    figure.classList.add("drawn");
    figure.dataset.source = source;
  }
}

/** Draws every diagram again, as after the theme changed: a drawing keeps
 *  the colours it was drawn in. */
export async function redrawDiagrams(root: HTMLElement): Promise<void> {
  for (const figure of root.querySelectorAll<HTMLElement>("figure.diagram.drawn")) {
    figure.textContent = figure.dataset.source ?? "";
    figure.classList.remove("drawn");
  }
  await drawDiagrams(root);
}

async function load(): Promise<Mermaid> {
  loading ??= import("mermaid").then((module) => module.default);
  const mermaid = await loading;
  const style = getComputedStyle(document.documentElement);
  const token = (name: string) => style.getPropertyValue(name).trim();
  const wanted = document.documentElement.dataset.theme ?? "";
  if (wanted !== theme) {
    theme = wanted;
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      theme: "base",
      fontFamily: token("--font-reading"),
      themeVariables: {
        darkMode: wanted !== "warm-white",
        background: token("--bg"),
        fontFamily: token("--font-reading"),
        fontSize: token("--body-size"),
        primaryColor: token("--surface-code"),
        primaryTextColor: token("--text"),
        primaryBorderColor: token("--surface-code"),
        secondaryColor: token("--surface-header"),
        tertiaryColor: token("--bg-sunken"),
        lineColor: token("--text-3"),
        textColor: token("--text"),
        mainBkg: token("--surface-code"),
        clusterBkg: token("--bg-sunken"),
        clusterBorder: token("--bg-sunken"),
        edgeLabelBackground: token("--bg"),
        noteBkgColor: token("--surface-header"),
        noteTextColor: token("--text"),
        noteBorderColor: token("--surface-header"),
        actorBkg: token("--surface-code"),
        actorBorder: token("--surface-code"),
        actorTextColor: token("--text"),
        signalColor: token("--text-2"),
        signalTextColor: token("--text"),
      },
    });
  }
  return mermaid;
}
