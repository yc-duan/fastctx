// A page's frame: cards that hold things, a grid that lays them out and
// folds to one column on a phone, and a number said large with what it is.
//
//   <w-grid columns="3"> … </w-grid>
//   <w-card title="训练" note="lab 跑了 4 小时"> … </w-card>
//   <w-stat label="验证损失" value="0.412" delta="-0.031" good="down"></w-stat>

import { BOUND, PageElement, escape, sheet } from "../element";

export class Grid extends PageElement {
  static observedAttributes = ["columns", "gap"];
  protected static override styles = sheet(`
    .grid {
      display: grid;
      gap: var(--gap, var(--space-4));
      grid-template-columns: repeat(var(--columns, 2), minmax(0, 1fr));
    }
    @media (max-width: 640px) {
      .grid { grid-template-columns: minmax(0, 1fr); }
    }
  `);

  protected draw(): void {
    const columns = Math.max(1, Math.min(6, Math.round(this.number("columns", 2))));
    this.shadow.innerHTML = `<div class="grid" style="--columns:${columns}"><slot></slot></div>`;
  }
}

export class Card extends PageElement {
  static observedAttributes = ["title", "note"];
  protected static override styles = sheet(`
    .card {
      height: 100%;
      padding: var(--space-4) var(--space-5);
      border-radius: var(--radius-l);
      background: var(--bg-elevated);
      box-shadow: 0 0 0 1px var(--hairline);
    }
    .head {
      display: flex;
      align-items: baseline;
      justify-content: space-between;
      gap: var(--space-3);
      margin-bottom: var(--space-3);
    }
    .title {
      margin: 0;
      font-family: var(--font-emphasis);
      font-weight: 400;
      font-size: var(--size-h3);
      color: var(--text-strong);
    }
    .note { font-size: var(--body-size); color: var(--text-3); }
  `);

  protected draw(): void {
    const title = this.getAttribute("title");
    const note = this.getAttribute("note");
    const head =
      title || note
        ? `<div class="head">${title ? `<h2 class="title">${escape(title)}</h2>` : "<span></span>"}${note ? `<span class="note">${escape(note)}</span>` : ""}</div>`
        : "";
    this.shadow.innerHTML = `<section class="card">${head}<slot></slot></section>`;
  }
}

export class Stat extends PageElement {
  /** The number: its attribute, or what a bound tool answered. */
  private value(): string {
    if (this.hasAttribute("value")) return this.getAttribute("value")!;
    if (this.loading) return "…";
    const data = this.data;
    if (this.failed(data) !== null) return "—";
    return typeof data === "number" ? String(Number(data.toPrecision(6))) : typeof data === "string" ? data : "—";
  }

  /** Why a bound value is missing: the World's refusal, or an answer
   *  that holds no number or text where `pick` pointed. */
  private failed(data: unknown): string | null {
    if (this.hasAttribute("value") || this.loading) return null;
    if (data && typeof data === "object" && "error" in data) return String((data as { error: unknown }).error);
    if (this.getAttribute("tool") && typeof data !== "number" && typeof data !== "string") return `pick: ${this.getAttribute("pick") ?? "the answer"} is not a number or text`;
    return null;
  }

  static observedAttributes = ["label", "value", "unit", "delta", "good", "note", ...BOUND];
  protected static override styles = sheet(`
    .value {
      display: flex;
      align-items: baseline;
      gap: var(--space-1);
      margin-top: var(--space-1);
      font-family: var(--font-emphasis);
      font-size: calc(var(--body-size) * 1.9);
      line-height: 1.1;
      color: var(--text-strong);
      font-variant-numeric: tabular-nums;
    }
    .unit { font-size: var(--body-size); color: var(--text-2); }
    .delta { margin-left: var(--space-2); font-size: var(--body-size); font-family: var(--font-reading); }
    .better { color: var(--alive); }
    .worse { color: var(--wrong); }
    .note { margin-top: var(--space-1); font-size: var(--body-size); color: var(--text-3); }
    .note.wrong { color: var(--wrong); }
  `);

  protected draw(): void {
    const delta = this.getAttribute("delta");
    // Whether a change is for the better: up, or down where `good="down"`.
    let tone = "";
    if (delta) {
      const sign = delta.trim().startsWith("-") ? -1 : delta.trim().startsWith("+") || Number(delta) > 0 ? 1 : 0;
      const good = this.getAttribute("good") === "down" ? -1 : 1;
      tone = sign === 0 ? "" : sign === good ? "better" : "worse";
    }
    this.shadow.innerHTML = `
      <div class="label">${escape(this.getAttribute("label"))}</div>
      <div class="value"><span>${escape(this.value())}</span>${this.getAttribute("unit") ? `<span class="unit">${escape(this.getAttribute("unit"))}</span>` : ""}${delta ? `<span class="delta ${tone}">${escape(delta)}</span>` : ""}</div>
      ${(() => {
        const why = this.failed(this.data);
        if (why !== null) return `<div class="note wrong" role="status">${escape(why)}</div>`;
        return this.getAttribute("note") ? `<div class="note">${escape(this.getAttribute("note"))}</div>` : "";
      })()}`;
  }
}
