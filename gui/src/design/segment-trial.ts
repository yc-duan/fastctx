// The trial of the switch between a few choices (`segment-trial.css`), in a
// development build only: `?seg=a` (or `b`, `c`, `now`) in the address
// chooses a look and brings a small chooser to the window's foot, from
// which the looks are tried on any page; what was chosen is kept for the
// visit.

import "./segment-trial.css";

const LOOKS = ["now", "a", "b", "c"] as const;
type Look = (typeof LOOKS)[number];
const KEPT = "seg-trial";

const NAMES: Record<Look, [string, string]> = {
  now: ["现在", "Now"],
  a: ["A 墨线", "A stroke"],
  b: ["B 描圈", "B ring"],
  c: ["C 灰底", "C grey"],
};

function kept(): Look | null {
  try {
    const value = sessionStorage.getItem(KEPT) as Look | null;
    return value && LOOKS.includes(value) ? value : null;
  } catch {
    return null;
  }
}

function keep(look: Look | null): void {
  try {
    if (look) sessionStorage.setItem(KEPT, look);
    else sessionStorage.removeItem(KEPT);
  } catch {
    // Kept for this page only.
  }
}

export function installSegmentTrial(english: boolean): void {
  const asked = new URLSearchParams(location.search).get("seg") as Look | null;
  let look = asked && LOOKS.includes(asked) ? asked : kept();
  if (!look) return;
  const root = document.documentElement;
  const bar = document.createElement("div");
  bar.className = "seg-trial";
  const draw = () => {
    if (look === "now" || !look) delete root.dataset.seg;
    else root.dataset.seg = look;
    bar.innerHTML = `<span>${english ? "Switch" : "选择器方案"}</span><div class="segmented" role="radiogroup">${LOOKS.map(
      (one) => `<button type="button" class="segment${one === look ? " chosen" : ""}" role="radio" aria-checked="${one === look}" data-look="${one}">${NAMES[one][english ? 1 : 0]}</button>`,
    ).join("")}</div><button type="button" class="seg-trial-shut" aria-label="close" title="close">×</button>`;
  };
  bar.addEventListener("click", (event) => {
    const target = event.target as HTMLElement;
    if (target.closest(".seg-trial-shut")) {
      look = null;
      keep(null);
      delete root.dataset.seg;
      bar.remove();
      return;
    }
    const chosen = target.closest<HTMLElement>("[data-look]")?.dataset.look as Look | undefined;
    if (!chosen || chosen === look) return;
    look = chosen;
    keep(look);
    draw();
  });
  keep(look);
  draw();
  document.body.append(bar);
}
