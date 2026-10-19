// Numbers over something as a line or bars, drawn in the pen's inks: clay
// first, then sky, sage, rose, graphite. Axes are marked at round numbers;
// moving over the chart says the values at that point. It draws again when
// its width changes.
//
//   <w-chart x="step" y="loss,val_loss" labels="训练,验证" unit="" height="220">
//     <script type="application/json">[{"step":0,"loss":2.3,"val_loss":2.4}, …]</script>
//   </w-chart>
//
// Data is an array of objects (x and y name their keys; y defaults to every
// numeric key but x), or { x: [...], series: [{ name, values: [...] }] }.

import { BOUND, PageElement, escape, sheet } from "../element";

const INKS = ["--pen-clay", "--pen-sky", "--pen-sage", "--pen-rose", "--pen-graphite"];

interface Series {
  name: string;
  values: (number | null)[];
}

interface Shaped {
  x: (number | string)[];
  series: Series[];
}

export class Chart extends PageElement {
  static observedAttributes = ["type", "x", "y", "labels", "unit", "height", ...BOUND];
  protected static override styles = sheet(`
    .chart { position: relative; }
    svg { display: block; width: 100%; overflow: visible; }
    .tick { font-size: 11px; fill: var(--text-3); font-variant-numeric: tabular-nums; font-family: var(--font-reading); }
    .grid { stroke: var(--divider); stroke-width: 1; }
    .axis { stroke: var(--hairline); stroke-width: 1; }
    .rule { stroke: var(--rule); stroke-width: 1; }
    .legend { display: flex; flex-wrap: wrap; gap: var(--space-1) var(--space-4); margin-bottom: var(--space-2); font-size: var(--size-fine); color: var(--text-2); }
    .legend i { display: inline-block; width: 10px; height: 2px; margin-right: 6px; vertical-align: middle; border-radius: 1px; }
    .tip {
      position: absolute;
      top: 0;
      pointer-events: none;
      padding: 6px var(--space-2);
      border-radius: var(--radius-s);
      background: var(--bg-elevated);
      box-shadow: var(--shadow-pop), 0 0 0 1px var(--hairline);
      font-size: 12px;
      line-height: 1.5;
      white-space: nowrap;
      color: var(--text);
    }
    .tip b { font-weight: 400; color: var(--text-3); }
    .tip i { display: inline-block; width: 8px; height: 2px; margin-right: 6px; vertical-align: middle; }
    .empty { padding: var(--space-3) 0; font-size: var(--body-size); color: var(--text-3); }
  `);
  private width = 0;
  private readonly sizes = new ResizeObserver(([entry]) => {
    const width = Math.round(entry?.contentRect.width ?? 0);
    if (width !== this.width) {
      this.width = width;
      this.draw();
    }
  });

  override connectedCallback(): void {
    this.sizes.observe(this);
    super.connectedCallback();
  }

  override disconnectedCallback(): void {
    this.sizes.disconnect();
    super.disconnectedCallback();
  }

  protected draw(): void {
    if (this.loading) {
      this.shadow.innerHTML = `<div class="empty">…</div>`;
      return;
    }
    const data = this.data;
    if (data && typeof data === "object" && "error" in data && typeof (data as { error: unknown }).error === "string") {
      this.shadow.innerHTML = `<div class="empty wrong">${escape((data as { error: string }).error)}</div>`;
      return;
    }
    const shaped = shape(data, this.getAttribute("x"), this.getAttribute("y"), this.getAttribute("labels"));
    if (!shaped.x.length || !shaped.series.length || !this.width) {
      this.shadow.innerHTML = `<div class="empty">—</div>`;
      return;
    }
    const bars = this.getAttribute("type") === "bar";
    const unit = this.getAttribute("unit") ?? "";
    const width = this.width;
    const height = this.number("height", 220);
    const left = 44;
    const right = 8;
    const top = 8;
    const bottom = 24;
    const plotW = Math.max(10, width - left - right);
    const plotH = Math.max(10, height - top - bottom);

    const values = shaped.series.flatMap((series) => series.values.filter((v): v is number => v !== null));
    let low = Math.min(...values);
    let high = Math.max(...values);
    if (bars) {
      low = Math.min(0, low);
      high = Math.max(0, high);
    }
    if (low === high) {
      low -= 1;
      high += 1;
    }
    const ticks = niceTicks(low, high, 5);
    low = Math.min(low, ticks[0]!);
    high = Math.max(high, ticks[ticks.length - 1]!);
    const y = (v: number) => top + plotH - ((v - low) / (high - low)) * plotH;

    const numericX = shaped.x.every((v) => typeof v === "number");
    const xs = shaped.x as number[];
    const xLow = numericX ? Math.min(...xs) : 0;
    const xHigh = numericX ? Math.max(...xs) : shaped.x.length - 1;
    const slot = plotW / shaped.x.length;
    const x = (index: number) =>
      bars
        ? left + slot * index + slot / 2
        : numericX
          ? left + (xHigh === xLow ? plotW / 2 : ((xs[index]! - xLow) / (xHigh - xLow)) * plotW)
          : left + (shaped.x.length === 1 ? plotW / 2 : (index / (shaped.x.length - 1)) * plotW);

    const grid = ticks
      .map((tick) => `<line class="grid" x1="${left}" x2="${left + plotW}" y1="${y(tick)}" y2="${y(tick)}"/><text class="tick" x="${left - 6}" y="${y(tick) + 4}" text-anchor="end">${escape(compact(tick))}</text>`)
      .join("");
    // As many x labels as fit, evenly chosen.
    const room = Math.max(1, Math.floor(plotW / 64));
    const every = Math.max(1, Math.ceil(shaped.x.length / room));
    const xLabels = shaped.x
      .map((value, index) => (index % every === 0 ? `<text class="tick" x="${x(index)}" y="${top + plotH + 16}" text-anchor="middle">${escape(typeof value === "number" ? compact(value) : value)}</text>` : ""))
      .join("");

    let marks = "";
    if (bars) {
      const groupW = slot * 0.72;
      const barW = groupW / shaped.series.length;
      marks = shaped.series
        .map((series, s) =>
          series.values
            .map((v, index) => {
              if (v === null) return "";
              const x0 = left + slot * index + (slot - groupW) / 2 + barW * s;
              const y0 = y(Math.max(0, v));
              const h = Math.abs(y(v) - y(0));
              return `<rect x="${x0}" y="${y0}" width="${Math.max(1, barW - 1)}" height="${Math.max(0.5, h)}" rx="1.5" fill="var(${INKS[s % INKS.length]})"/>`;
            })
            .join(""),
        )
        .join("");
    } else {
      marks = shaped.series
        .map((series, s) => {
          let path = "";
          let pen = false;
          series.values.forEach((v, index) => {
            if (v === null) {
              pen = false;
              return;
            }
            path += `${pen ? "L" : "M"}${x(index).toFixed(1)},${y(v).toFixed(1)}`;
            pen = true;
          });
          const dots = shaped.x.length <= 40 ? series.values.map((v, index) => (v === null ? "" : `<circle cx="${x(index)}" cy="${y(v)}" r="2" fill="var(${INKS[s % INKS.length]})"/>`)).join("") : "";
          return `<path d="${path}" fill="none" stroke="var(${INKS[s % INKS.length]})" stroke-width="1.75" stroke-linejoin="round" stroke-linecap="round"/>${dots}`;
        })
        .join("");
    }
    const legend =
      shaped.series.length > 1
        ? `<div class="legend">${shaped.series.map((series, s) => `<span><i style="background:var(${INKS[s % INKS.length]})"></i>${escape(series.name)}</span>`).join("")}</div>`
        : "";
    this.shadow.innerHTML = `${legend}<div class="chart"><svg viewBox="0 0 ${width} ${height}" height="${height}" role="img" aria-label="${escape(shaped.series.map((series) => series.name).join(", "))}">
      ${grid}<line class="axis" x1="${left}" x2="${left + plotW}" y1="${top + plotH}" y2="${top + plotH}"/>${xLabels}${marks}<line class="rule" x1="0" x2="0" y1="${top}" y2="${top + plotH}" visibility="hidden"/>
    </svg><div class="tip" hidden></div></div>`;

    // Moving over it says the values at the nearest point.
    const svg = this.shadow.querySelector("svg")!;
    const rule = svg.querySelector<SVGLineElement>(".rule")!;
    const tip = this.shadow.querySelector<HTMLElement>(".tip")!;
    svg.addEventListener("pointermove", (event) => {
      const box = svg.getBoundingClientRect();
      const px = ((event.clientX - box.left) / box.width) * width;
      let nearest = 0;
      for (let index = 1; index < shaped.x.length; index += 1) {
        if (Math.abs(x(index) - px) < Math.abs(x(nearest) - px)) nearest = index;
      }
      const at = x(nearest);
      rule.setAttribute("x1", String(at));
      rule.setAttribute("x2", String(at));
      rule.setAttribute("visibility", "visible");
      const label = shaped.x[nearest]!;
      tip.innerHTML = `<b>${escape(typeof label === "number" ? compact(label) : label)}</b>${shaped.series
        .map((series, s) => `<div><i style="background:var(${INKS[s % INKS.length]})"></i>${escape(series.name)} ${escape(series.values[nearest] === null ? "—" : `${round(series.values[nearest]!)}${unit}`)}</div>`)
        .join("")}`;
      tip.hidden = false;
      const tipW = tip.offsetWidth;
      tip.style.left = `${Math.min(width - tipW, Math.max(0, (at / width) * box.width + 10))}px`;
    });
    svg.addEventListener("pointerleave", () => {
      rule.setAttribute("visibility", "hidden");
      tip.hidden = true;
    });
  }
}

function shape(data: unknown, xKey: string | null, yKeys: string | null, labels: string | null): Shaped {
  const names = labels?.split(",").map((name) => name.trim()) ?? [];
  if (data && typeof data === "object" && !Array.isArray(data) && Array.isArray((data as Shaped).x)) {
    const given = data as Shaped;
    return {
      x: given.x,
      series: (given.series ?? []).map((series, s) => ({ name: names[s] ?? series.name, values: series.values.map(numberOr) })),
    };
  }
  if (!Array.isArray(data)) return { x: [], series: [] };
  const rows = data.filter((row): row is Record<string, unknown> => row !== null && typeof row === "object");
  const first = rows[0] ?? {};
  const xName = xKey ?? Object.keys(first)[0] ?? "";
  const yNames = yKeys
    ? yKeys.split(",").map((key) => key.trim())
    : Object.keys(first).filter((key) => key !== xName && typeof first[key] === "number");
  return {
    x: rows.map((row) => (typeof row[xName] === "number" ? (row[xName] as number) : String(row[xName] ?? ""))),
    series: yNames.map((key, s) => ({ name: names[s] ?? key, values: rows.map((row) => numberOr(row[key])) })),
  };
}

function numberOr(value: unknown): number | null {
  const number = typeof value === "number" ? value : typeof value === "string" && value.trim() !== "" ? Number(value) : NaN;
  return Number.isFinite(number) ? number : null;
}

/** Round numbers to mark an axis from `low` to `high` with: 1, 2 or 5 times
 *  a power of ten apart. */
function niceTicks(low: number, high: number, count: number): number[] {
  const raw = (high - low) / Math.max(1, count - 1);
  const power = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 5, 10].map((m) => m * power).find((s) => s >= raw) ?? 10 * power;
  const start = Math.floor(low / step) * step;
  const ticks: number[] = [];
  for (let tick = start; tick <= high + step * 0.5; tick += step) ticks.push(Number(tick.toPrecision(12)));
  return ticks;
}

/** 1200 as 1.2k, 3400000 as 3.4M; small numbers as they are. */
function compact(value: number): string {
  const size = Math.abs(value);
  if (size >= 1e9) return `${round(value / 1e9)}G`;
  if (size >= 1e6) return `${round(value / 1e6)}M`;
  if (size >= 1e4) return `${round(value / 1e3)}k`;
  return round(value);
}

function round(value: number): string {
  return String(Number(value.toPrecision(4)));
}
