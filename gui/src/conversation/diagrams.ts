// Diagrams are drawn once the words around them are done: a half-written
// diagram is not one. Mermaid loads the first time a diagram needs it,
// takes its colours from the page's tokens, and runs strict, so a diagram
// can never bring script or HTML of its own. One it cannot draw keeps its
// source in view.

type Mermaid = typeof import("mermaid").default;

let loading: Promise<Mermaid> | null = null;
let theme = "";
let serial = 0;

export async function drawDiagrams(root: HTMLElement): Promise<void> {
  const waiting = [...root.querySelectorAll<HTMLElement>("figure.diagram[data-mermaid]:not(.drawn):not(.unreadable)")].filter(
    (figure) => !figure.closest(".writing"),
  );
  if (!waiting.length) return;
  const mermaid = await load();
  for (const figure of waiting) {
    if (figure.classList.contains("drawn") || !figure.isConnected) continue;
    const source = figure.textContent ?? "";
    serial += 1;
    try {
      const { svg } = await mermaid.render(`diagram-${serial}`, source);
      figure.innerHTML = svg;
      figure.classList.add("drawn");
      figure.dataset.source = source;
    } catch {
      figure.classList.add("unreadable");
    }
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
