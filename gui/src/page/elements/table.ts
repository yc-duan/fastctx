// Rows of data as a table: numbers line up on the right in figures of one
// width, a header sorts by its column when pressed.
//
//   <w-table columns="run:运行,loss:损失,acc:准确率">
//     <script type="application/json">[{"run":"a","loss":0.41,"acc":0.88}]</script>
//   </w-table>
//
// Data is an array of objects, or { columns: [...], rows: [[...]] }.

import { BOUND, PageElement, escape, sheet } from "../element";

type Row = Record<string, unknown>;

export class Table extends PageElement {
  static observedAttributes = ["columns", ...BOUND];
  protected static override styles = sheet(`
    .wrap { overflow-x: auto; scrollbar-width: thin; }
    table { width: 100%; border-collapse: collapse; font-size: var(--body-size); }
    th, td { padding: 6px var(--space-3); text-align: left; border-bottom: 1px solid var(--divider); white-space: nowrap; }
    th { font-weight: 400; font-size: var(--body-size); color: var(--text-3); }
    th button { padding: 0; color: inherit; }
    th button[aria-sort]::after { content: " ↓"; }
    th button[aria-sort="ascending"]::after { content: " ↑"; }
    td.number, th.number { text-align: right; font-variant-numeric: tabular-nums; }
    tbody tr:hover td { background: var(--hairline); }
    .empty { padding: var(--space-3); color: var(--text-3); font-size: var(--body-size); }
  `);
  private sortBy: { key: string; up: boolean } | null = null;

  constructor() {
    super();
    this.shadow.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-sort]");
      if (!button) return;
      const key = button.dataset.sort!;
      this.sortBy = this.sortBy?.key === key ? { key, up: !this.sortBy.up } : { key, up: true };
      this.draw();
    });
  }

  protected draw(): void {
    if (this.loading) {
      this.shadow.innerHTML = `<div class="empty">…</div>`;
      return;
    }
    const { columns, rows, error } = shape(this.data, this.getAttribute("columns"));
    if (error) {
      this.shadow.innerHTML = `<div class="empty wrong">${escape(error)}</div>`;
      return;
    }
    if (!rows.length) {
      this.shadow.innerHTML = `<div class="empty">—</div>`;
      return;
    }
    const numeric = new Set(columns.filter(({ key }) => rows.every((row) => row[key] === null || row[key] === undefined || typeof row[key] === "number")).map(({ key }) => key));
    const sorted = [...rows];
    if (this.sortBy) {
      const { key, up } = this.sortBy;
      sorted.sort((a, b) => {
        const x = a[key];
        const y = b[key];
        const order = typeof x === "number" && typeof y === "number" ? x - y : String(x ?? "").localeCompare(String(y ?? ""));
        return up ? order : -order;
      });
    }
    const head = columns
      .map(({ key, label }) => {
        const sort = this.sortBy?.key === key ? ` aria-sort="${this.sortBy.up ? "ascending" : "descending"}"` : "";
        return `<th class="${numeric.has(key) ? "number" : ""}"><button type="button" data-sort="${escape(key)}"${sort}>${escape(label)}</button></th>`;
      })
      .join("");
    const body = sorted
      .map((row) => `<tr>${columns.map(({ key }) => `<td class="${numeric.has(key) ? "number" : ""}">${escape(cell(row[key]))}</td>`).join("")}</tr>`)
      .join("");
    this.shadow.innerHTML = `<div class="wrap"><table><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table></div>`;
  }
}

/** The columns and rows of what was given, in either shape. */
function shape(data: unknown, named: string | null): { columns: { key: string; label: string }[]; rows: Row[]; error?: string } {
  if (data && typeof data === "object" && "error" in data && typeof (data as { error: unknown }).error === "string") {
    return { columns: [], rows: [], error: (data as { error: string }).error };
  }
  let rows: Row[] = [];
  let keys: string[] = [];
  if (Array.isArray(data)) {
    rows = data.filter((row): row is Row => row !== null && typeof row === "object");
    keys = [...new Set(rows.flatMap((row) => Object.keys(row)))];
  } else if (data && typeof data === "object" && Array.isArray((data as { rows?: unknown }).rows)) {
    const given = data as { columns?: unknown[]; rows: unknown[][] };
    keys = (given.columns ?? []).map(String);
    rows = given.rows.map((row) => Object.fromEntries(keys.map((key, index) => [key, row[index]])));
  }
  const columns = named
    ? named.split(",").map((part) => {
        const [key, label] = part.split(":");
        return { key: key!.trim(), label: (label ?? key!).trim() };
      })
    : keys.map((key) => ({ key, label: key }));
  return { columns, rows };
}

function cell(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "number") return Number.isInteger(value) ? String(value) : String(Number(value.toPrecision(6)));
  if (typeof value === "object") return JSON.stringify(value);
  return String(value);
}
