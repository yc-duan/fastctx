import "../design/fonts.css";
import "../design/tokens.css";
import "../design/base.css";
import "./longview.css";
import "./specimen.css";
import { applyTheme } from "../theme";
import { AnchorLayout, MemoryLines, TextFold, chineseViewer } from "./index";
import type { FoldFrame, LineChange, LineCursor, LineExtent, LineMatch, LineSource } from "./index";

class SliceSource implements LineSource {
  requests: { start: number; count: number; bytes: number; cancelled: boolean }[] = [];
  private listeners = new Set<(change: LineChange) => void>();
  constructor(public extent: LineExtent, readonly mode = "plain") {}
  async read(start: number, count: number, signal: AbortSignal): Promise<readonly string[]> {
    const record = { start, count, bytes: 0, cancelled: false };
    this.requests.push(record);
    try {
      const response = await fetch(`/__longview/lines?length=${this.extent.length}&start=${start}&count=${count}&mode=${this.mode}`, { signal });
      if (!response.ok) throw new Error(`${response.status} ${await response.text()}`);
      const text = await response.text();
      record.bytes = new TextEncoder().encode(text).byteLength;
      return JSON.parse(text) as string[];
    } finally { record.cancelled = signal.aborted; }
  }
  async find(query: string, from: LineCursor, direction: 1 | -1, signal: AbortSignal): Promise<LineMatch | null> {
    const response = await fetch(`/__longview/find?length=${this.extent.length}&start=${from.line}&direction=${direction}&query=${encodeURIComponent(query)}&mode=${this.mode}${from.offset === undefined ? "" : `&offset=${from.offset}`}`, { signal });
    if (!response.ok) throw new Error(`${response.status} ${await response.text()}`);
    return response.json() as Promise<LineMatch | null>;
  }
  subscribe(changed: (change: LineChange) => void): () => void { this.listeners.add(changed); return () => this.listeners.delete(changed); }
  grow(length: number, complete: boolean): void {
    const from = Math.max(0, this.extent.length - 1);
    this.extent = { length, complete };
    for (const changed of this.listeners) changed({ from });
  }
}

applyTheme(() => {});
const params = new URLSearchParams(location.search);
const chinese = params.get("lang") === "zh";
document.documentElement.lang = chinese ? "zh-CN" : "en";
const scroller = document.querySelector<HTMLElement>("#conversation")!;
const content = document.querySelector<HTMLElement>("#content")!;
const layout = new AnchorLayout(scroller);
const log = new SliceSource({ length: 100000, complete: true });
const million = new SliceSource({ length: 1000000, complete: true });
const unicode = new SliceSource({ length: 40, complete: true }, "unicode");
const growing = new SliceSource({ length: 6, complete: false });
const thought = new MemoryLines(Array.from({ length: 40 }, (_, index) => `${index + 1}. Compare the evidence. 中文与 emoji 🧑🏽‍🚀 stay intact.`));
const frames: FoldFrame[] = [];
function prose(text: string): void { const p = document.createElement("p"); p.textContent = text; content.append(p); }
prose(chinese ? "先看结果，再打开细节。" : "The result is ready. Open a line to read its details.");
const folds: TextFold[] = [];
function fold(heading: string, source: LineSource, previewLines = 12): TextFold {
  const made = new TextFold({
    heading, source, layout, previewLines, onFrame: (frame) => frames.push(frame),
    ...(chinese ? { less: "− 收起", more: (count: number, complete: boolean) => `+ ${count.toLocaleString()} 行${complete ? "" : "…"}`, fullScreen: "整屏看", viewerWords: chineseViewer } : {}),
  });
  folds.push(made); content.append(made.element); return made;
}
fold(chinese ? "思考 · 比较证据" : "Thinking · Compare evidence", thought);
prose(chinese ? "运行已结束，退出 0。" : "The command finished with exit 0.");
fold(chinese ? "运行 · 100,000 行" : "Run · 100,000 lines", log);
prose(chinese ? "下面继续阅读对话。" : "The conversation continues below the output.");
fold(chinese ? "运行 · 1,000,000 行" : "Run · 1,000,000 lines", million);
fold(chinese ? "读取 · Unicode 长行" : "Read · Long Unicode line", unicode);
fold(chinese ? "运行中 · 持续输出" : "Running · Growing output", growing);
const grow = document.createElement("button");
grow.className = "lv-specimen-grow";
grow.textContent = chinese ? "追加 100 行" : "Append 100 lines";
grow.addEventListener("click", () => growing.grow(growing.extent.length + 100, false));
content.append(grow);
const short = fold(chinese ? "读取 · 短内容" : "Read · Short content", thought, 35);
for (let index = 0; index < 18; index++) prose(chinese ? `对话继续 · ${index + 1}` : `Conversation continues · ${index + 1}`);
document.querySelector("#summary")!.textContent = chinese ? "按需读取" : "Slice reads";

// A public specimen handle lets external probes measure actual browser frames.
Object.assign(window, { longview: { folds, log, million, unicode, growing, short, frames, scroller, layout } });
if (params.get("scene") === "open") {
  await folds[1].setOpen(true);
  folds[1].heading.scrollIntoView({ block: "start" });
} else if (params.get("scene") === "full") {
  await folds[1].setOpen(true);
  await folds[1].setFull(true);
  folds[1].element.scrollIntoView({ block: "center" });
} else if (params.get("scene") === "viewer") {
  await folds[1].setOpen(true);
  (folds[1].element.querySelector(".lv-corner") as HTMLButtonElement).click();
}
