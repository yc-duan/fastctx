// The paper and the pen: SVG filters that give a drawn shape the grain of
// a marker or a dry brush on paper. A high-frequency noise, turned to alpha,
// is taken out of the shape (the grain); for the dry brush and the ruling
// pen a low-frequency noise also displaces it a little (the waver). The
// marker has no waver: its shapes already carry the hand in their outline
// (`shapes.ts`), and a jittering edge on an even line reads as a child's
// scribble. Each pen has a light form without the waver, for shapes that
// move or are many. The felt marker of the bars is the one other: its
// noise is stretched along the stroke into the streaks the felt leaves,
// and a fine, quick displacement frays its edge the way ink bleeds into
// paper fibre, too small and too quick to read as a wobble.
//
// Decoration only: text is never drawn through these, so it stays sharp.

interface Pen {
  grain: { frequency: number; seed: number; amount: number };
  waver: { frequency: number; seed: number; scale: number } | null;
  /** Streaks along x: a noise far slower along the stroke than across. */
  streak?: { frequency: [number, number]; seed: number; amount: number };
}

const pens: Record<string, Pen> = {
  "pen-marker": { grain: { frequency: 0.55, seed: 7, amount: 0.3 }, waver: null },
  "pen-dry": { grain: { frequency: 0.9, seed: 4, amount: 0.55 }, waver: { frequency: 0.18, seed: 9, scale: 1.8 } },
  "pen-rule": { grain: { frequency: 0.72, seed: 5, amount: 0.12 }, waver: { frequency: 0.07, seed: 3, scale: 0.6 } },
  "pen-marker-lite": { grain: { frequency: 0.55, seed: 7, amount: 0.3 }, waver: null },
  "pen-dry-lite": { grain: { frequency: 0.9, seed: 4, amount: 0.55 }, waver: null },
  "pen-rule-lite": { grain: { frequency: 0.72, seed: 5, amount: 0.12 }, waver: null },
  "pen-felt": {
    grain: { frequency: 0.85, seed: 5, amount: 0.13 },
    streak: { frequency: [0.018, 0.62], seed: 13, amount: 0.25 },
    waver: { frequency: 0.95, seed: 6, scale: 0.75 },
  },
};

export type PenName = keyof typeof pens;

function filter(id: string, pen: Pen): string {
  const { grain, waver, streak } = pen;
  const streaked = streak
    ? `<feTurbulence type="fractalNoise" baseFrequency="${streak.frequency.join(" ")}" numOctaves="2" seed="${streak.seed}" result="streak"/><feColorMatrix in="streak" type="luminanceToAlpha" result="streakAlpha"/><feComposite in="SourceGraphic" in2="streakAlpha" operator="arithmetic" k1="0" k2="1" k3="${-streak.amount}" k4="0" result="streaked"/>`
    : "";
  const inked = `${streaked}<feTurbulence type="fractalNoise" baseFrequency="${grain.frequency}" numOctaves="2" seed="${grain.seed}" result="grain"/><feColorMatrix in="grain" type="luminanceToAlpha" result="grainAlpha"/><feComposite in="${streak ? "streaked" : "SourceGraphic"}" in2="grainAlpha" operator="arithmetic" k1="0" k2="1" k3="${-grain.amount}" k4="0" result="inked"/>`;
  const wavered = waver
    ? `<feTurbulence type="fractalNoise" baseFrequency="${waver.frequency}" numOctaves="2" seed="${waver.seed}" result="waver"/><feDisplacementMap in="inked" in2="waver" scale="${waver.scale}" xChannelSelector="R" yChannelSelector="G"/>`
    : "";
  return `<filter id="${id}" x="-8%" y="-8%" width="116%" height="116%" color-interpolation-filters="sRGB">${inked}${wavered}</filter>`;
}

/** The filters as SVG markup, for a drawing that carries its own. */
export function penDefs(): string {
  return Object.entries(pens)
    .map(([id, pen]) => filter(id, pen))
    .join("");
}

/** Puts the pens in the page once, for any drawing to use by id. */
export function installPens(): void {
  if (document.getElementById("pens")) return;
  const holder = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  holder.id = "pens";
  holder.setAttribute("width", "0");
  holder.setAttribute("height", "0");
  holder.setAttribute("aria-hidden", "true");
  holder.style.position = "absolute";
  holder.innerHTML = `<defs>${penDefs()}</defs>`;
  document.body.prepend(holder);
}
