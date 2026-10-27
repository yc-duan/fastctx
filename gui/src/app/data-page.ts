// What World keeps on this computer (`world-gui.md`, 设置; walkthrough item 7):
// one page, this machine only, never another's. At its top a hand-drawn
// bar in a colour for each kind (conversations, command output, cache,
// program) with how much each holds under it. Then the conversations, in
// three groups (listed here, removed but kept, guest rooms of conversations
// that live elsewhere), largest first; one opens to show its parts, several
// can be chosen and deleted together, on a red card. Then how long full
// command output stays after the model has its answer, the cache with the
// way to empty it. A phone keeps no conversation as their home, so there the
// page is its cache alone.

import { bytesSaid } from "../bytes";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { outline, seeded, type Point } from "../pen/shapes";
import { OUTPUT_DAYS, type ConversationSpace, type DataCall, type DataReply, type Group, type Space } from "../world/data";
import type { Confirm } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { detailOf } from "./problem";
import type { Page } from "./shell";

export interface DataWords {
  title: string;
  total(size: string): string;
  kinds: { conversations: string; command_output: string; cache: string; program: string; other: string };
  conversations: string;
  groups: Record<Group, string>;
  none: string;
  untitled: string;
  parts: { records: string; command_output: string; host(agent: string): string };
  hostGone(days: number | null): string;
  hostMissing: string;
  livesOn(machine: string): string;
  ago(at: Date, now: Date): string;
  delete(n: number, size: string): string;
  deleteTitle(n: number): string;
  deleteWhat(home: number, guests: number): string;
  deleteGo: string;
  deleted(n: number, size: string): string;
  refused(name: string, why: string): string;
  output: string;
  outputSaid: string;
  days(n: number): string;
  cache: string;
  cacheSaid: string;
  clear: string;
  cleared: string;
  /** How long Claude Code keeps a session nobody touched, as its login's
   *  settings say. */
  keeps(days: number): string;
  failed: string;
  reading: string;
}

export const zhData: DataWords = {
  title: "本机数据",
  total: (size) => `Fastctx 在这台电脑上占 ${size}`,
  kinds: { conversations: "对话", command_output: "命令输出", cache: "缓存", program: "程序", other: "其他" },
  conversations: "对话",
  groups: { listed: "在列", removed: "已移除", elsewhere: "别处的" },
  none: "这里没有",
  untitled: "未命名的对话",
  parts: { records: "历史与附件原件", command_output: "命令输出", host: (agent) => `${agent} 自己的记录` },
  hostGone: (days) => (days ? `Claude Code 已按 ${days} 天的保留期删掉` : "Claude Code 已自己删掉"),
  hostMissing: "不在它该在的地方",
  livesOn: (machine) => `家在 ${machine}`,
  ago: (at, now) => {
    const minutes = Math.round((now.getTime() - at.getTime()) / 60000);
    if (minutes < 60) return `${Math.max(1, minutes)} 分钟前`;
    if (minutes < 1440) return `${Math.round(minutes / 60)} 小时前`;
    return `${Math.round(minutes / 1440)} 天前`;
  },
  delete: (n, size) => `删除 ${n} 段，${size}`,
  deleteTitle: (n) => `删除 ${n} 段对话在这台电脑上的数据？`,
  deleteWhat: (home, guests) =>
    [
      home ? `${home} 段家在这台：它在这台上的一切都删掉，连同 Codex 或 Claude Code 自己的记录，侧栏里也就没了。` : "",
      guests ? `${guests} 段家在别处：只删它在这台留下的东西，它的家不碰。` : "",
      "删了找不回来。",
    ]
      .filter(Boolean)
      .join(""),
  deleteGo: "删除",
  deleted: (n, size) => `已删除 ${n} 段，腾出 ${size}`,
  refused: (name, why) => `${name} 没删：${why}`,
  output: "命令输出",
  outputSaid: "模型拿到之后，完整输出再留",
  days: (n) => `${n} 天`,
  cache: "缓存",
  cacheSaid: "点开过的文件、拍照留下的图与看过的对话；到期也会自己删",
  clear: "清空",
  cleared: "已清空",
  keeps: (days) => `Claude Code 会删掉 ${days} 天没动过的会话记录`,
  failed: "读不到",
  reading: "正在量",
};

export const enData: DataWords = {
  title: "Data on this computer",
  total: (size) => `Fastctx uses ${size} on this computer`,
  kinds: { conversations: "Conversations", command_output: "Command output", cache: "Cache", program: "Program", other: "Other" },
  conversations: "Conversations",
  groups: { listed: "Listed", removed: "Removed", elsewhere: "From elsewhere" },
  none: "None here",
  untitled: "Untitled conversation",
  parts: { records: "History and attached originals", command_output: "Command output", host: (agent) => `${agent}'s own record` },
  hostGone: (days) => (days ? `Deleted by Claude Code after ${days} days` : "Deleted by Claude Code"),
  hostMissing: "Not where it should be",
  livesOn: (machine) => `Lives on ${machine}`,
  ago: (at, now) => {
    const minutes = Math.round((now.getTime() - at.getTime()) / 60000);
    if (minutes < 60) return `${Math.max(1, minutes)} min ago`;
    if (minutes < 1440) return `${Math.round(minutes / 60)} h ago`;
    return `${Math.round(minutes / 1440)} d ago`;
  },
  delete: (n, size) => `Delete ${n}, ${size}`,
  deleteTitle: (n) => (n === 1 ? "Delete this conversation's data here?" : `Delete ${n} conversations' data here?`),
  deleteWhat: (home, guests) =>
    [
      home ? `${home} live here: everything of them here goes, their Codex or Claude Code record too, and they leave the sidebar. ` : "",
      guests ? `${guests} live elsewhere: only what they left here goes; their homes are untouched. ` : "",
      "This cannot be undone.",
    ].join(""),
  deleteGo: "Delete",
  deleted: (n, size) => `Deleted ${n}, freed ${size}`,
  refused: (name, why) => `${name} not deleted: ${why}`,
  output: "Command output",
  outputSaid: "After the model has its answer, keep the full output for",
  days: (n) => (n === 1 ? "1 day" : `${n} days`),
  cache: "Cache",
  cacheSaid: "Files opened, photos taken and conversations viewed; they expire on their own too",
  clear: "Empty",
  cleared: "Emptied",
  keeps: (days) => `Claude Code deletes sessions untouched for ${days} days`,
  failed: "Couldn't read",
  reading: "Measuring",
};

export interface DataPageOptions {
  words: DataWords;
  head(title: string, actions?: string): string;
  /** This computer's node; a phone has none. */
  data?: ((call: DataCall) => Promise<DataReply>) | undefined;
  /** This device's own cache of the conversations it viewed. On a computer
   *  it lies inside World's directory, so the node's count holds it. */
  cache?: { size(): Promise<number>; clear(): Promise<void> } | undefined;
  confirm: Confirm;
  now(): Date;
}

const KIND_ORDER = ["conversations", "command_output", "cache", "program", "other"] as const;

export class DataPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly body: HTMLElement;
  private space: Space | null = null;
  private problem: string | null = null;
  private deviceCache: number | null = null;
  private group: Group = "listed";
  private chosen = new Set<string>();
  private open = new Set<string>();
  private said: { text: string; detail: string } | null = null;

  constructor(private readonly options: DataPageOptions) {
    this.title = options.words.title;
    this.element = document.createElement("div");
    this.element.className = "settings-page data-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(options.words.title)}</h1>`)}<div class="settings data-body scroller"></div>`;
    this.body = this.element.querySelector<HTMLElement>(".data-body")!;
    this.body.addEventListener("click", (event) => void this.click(event));
    this.body.addEventListener("change", (event) => {
      const box = event.target as HTMLInputElement;
      if (box.dataset.choose === undefined) return;
      if (box.checked) this.chosen.add(box.dataset.choose);
      else this.chosen.delete(box.dataset.choose);
      this.draw();
    });
    this.draw();
    void this.read();
  }

  private async read(): Promise<void> {
    const { data, cache } = this.options;
    try {
      if (data) {
        const reply = await data({ do: "list" });
        if (reply.data === "listed") this.space = reply;
      }
      if (cache) this.deviceCache = await cache.size();
      this.problem = null;
    } catch (error) {
      this.problem = detailOf(error);
    }
    this.draw();
  }

  private draw(): void {
    const { words } = this.options;
    const space = this.space;
    let html: string;
    if (this.problem) html = `<p class="feature-note" title="${escape(this.problem)}">${escape(words.failed)}</p>`;
    else if (space) html = `${this.bar(space)}${this.conversations(space)}${this.output(space)}${this.cacheSection(null)}`;
    else if (!this.options.data && this.deviceCache !== null) html = this.cacheSection(this.deviceCache);
    else html = `<p class="feature-note">${escape(words.reading)}</p>`;
    redrawKeepingFocus(this.body, () => {
      this.body.innerHTML = html;
    });
  }

  /** The hand-drawn bar of everything here, a colour for each kind. */
  private bar(space: Space): string {
    const { words } = this.options;
    const sizes: Record<(typeof KIND_ORDER)[number], number> = {
      conversations: space.kinds.conversations + space.kinds.host_records,
      command_output: space.kinds.command_output,
      cache: space.kinds.cache,
      program: space.kinds.program,
      other: space.kinds.other,
    };
    const total = totalOf(space) || 1;
    const width = 600;
    let at = 4;
    const random = seeded("data-bar");
    const strokes = KIND_ORDER.filter((kind) => sizes[kind] > 0)
      .map((kind) => {
        const length = Math.max(6, ((width - 8) * sizes[kind]) / total);
        const points: Point[] = [];
        for (let x = at; x <= at + length; x += 8) points.push([x, 16 + Math.sin(x / 23) * 0.7]);
        points.push([at + length, 16]);
        at += length;
        return `<path class="data-ink data-${kind}" d="${outline(points, 14, random, { pressure: 0.16, taperStart: 0.02, taperEnd: 0.02 })}"/>`;
      })
      .join("");
    const legend = KIND_ORDER.filter((kind) => sizes[kind] > 0)
      .map((kind) => `<li><span class="data-swatch data-${kind}" aria-hidden="true"></span><span>${escape(words.kinds[kind])}</span><span class="data-size">${escape(size(sizes[kind]))}</span></li>`)
      .join("");
    return `<section class="data-top">
      <p class="data-total" title="${escape(space.root)}">${escape(words.total(size(totalOf(space))))}</p>
      <svg class="data-bar" viewBox="0 0 ${width} 32" preserveAspectRatio="none" aria-hidden="true"><g filter="url(#pen-marker-lite)">${strokes}</g></svg>
      <ul class="data-legend">${legend}</ul>
    </section>`;
  }

  private conversations(space: Space): string {
    const { words } = this.options;
    const now = this.options.now();
    const counts = (group: Group) => space.conversations.filter((one) => one.group === group).length;
    const tabs = (["listed", "removed", "elsewhere"] as const)
      .map((group) => `<button type="button" role="tab" class="data-tab${group === this.group ? " current" : ""}" aria-selected="${group === this.group}" data-group="${group}">${escape(words.groups[group])}<span class="count">${counts(group)}</span></button>`)
      .join("");
    const rows = space.conversations
      .filter((one) => one.group === this.group)
      .sort((a, b) => b.bytes - a.bytes)
      .map((one) => this.row(one, now))
      .join("");
    const chosen = space.conversations.filter((one) => this.chosen.has(one.name));
    const freed = chosen.reduce((sum, one) => sum + one.bytes, 0);
    return `<section class="home-section data-section"><h2 class="section-title">${escape(words.conversations)}</h2>
      <div class="data-tabs" role="tablist">${tabs}</div>
      ${rows ? `<ul class="data-rows">${rows}</ul>` : `<p class="feature-quiet">${escape(words.none)}</p>`}
      ${chosen.length ? `<div class="data-acts"><button class="removal-button" type="button" data-act="delete">${escape(words.delete(chosen.length, size(freed)))}</button></div>` : ""}
      ${this.said ? `<p class="feature-note" title="${escape(this.said.detail)}">${escape(this.said.text)}</p>` : ""}
    </section>`;
  }

  private row(one: ConversationSpace, now: Date): string {
    const { words } = this.options;
    const name = one.title ?? words.untitled;
    const open = this.open.has(one.name);
    const agent = one.agent === "claude" ? "Claude Code" : "Codex";
    const host =
      one.parts.host.kept === "kept"
        ? size(one.parts.host.bytes)
        : one.parts.host.kept === "gone_by_claude"
          ? words.hostGone(one.parts.host.days ?? null)
          : one.parts.host.kept === "missing"
            ? words.hostMissing
            : null;
    const parts = [
      [words.parts.records, size(one.parts.records)],
      [words.parts.command_output, size(one.parts.command_output)],
      ...(host ? [[words.parts.host(agent), host]] : []),
    ];
    const notes = [
      ...(one.parts.host.kept === "kept" && one.parts.host.days ? [words.keeps(one.parts.host.days)] : []),
      ...(one.group === "elsewhere" && one.lives_on ? [words.livesOn(one.lives_on)] : []),
    ];
    return `<li class="data-row${open ? " open" : ""}">
      <div class="data-row-line">
        <input type="checkbox" data-choose="${escape(one.name)}"${this.chosen.has(one.name) ? " checked" : ""}${one.held ? ` disabled title="${escape(one.held)}"` : ""} aria-label="${escape(name)}">
        <button class="data-row-open" type="button" data-open="${escape(one.name)}" aria-expanded="${open}">
          <span class="data-agent">${glyph(one.agent === "claude" ? "claude" : "codex", 16)}</span>
          <span class="data-name">${escape(name)}</span>
          ${one.town ? `<span class="tag">${escape(one.town)}</span>` : ""}
          <span class="data-when">${one.last_activity ? escape(words.ago(new Date(one.last_activity), now)) : ""}</span>
          <span class="data-size">${escape(size(one.bytes))}</span>
        </button>
      </div>
      ${open ? `<dl class="data-parts">${parts.map(([label, value]) => `<div><dt>${escape(label!)}</dt><dd>${escape(value!)}</dd></div>`).join("")}${notes.map((note) => `<p class="data-note">${escape(note)}</p>`).join("")}</dl>` : ""}
    </li>`;
  }

  private output(space: Space): string {
    const { words } = this.options;
    return `<section class="home-section data-section"><h2 class="section-title">${escape(words.output)}</h2>
      <p class="data-said">${escape(words.outputSaid)}</p>
      <div class="choice-options" role="radiogroup" aria-label="${escape(words.outputSaid)}">${OUTPUT_DAYS.map(
        (n) => `<button type="button" class="choice${n === space.output_days ? " chosen" : ""}" role="radio" aria-checked="${n === space.output_days}" data-days="${n}">${escape(words.days(n))}</button>`,
      ).join("")}</div></section>`;
  }

  private cacheSection(bytes: number | null): string {
    const { words } = this.options;
    // Its size is said here only where no bar above says it.
    const sized = bytes === null ? "" : `<span class="data-size">${escape(size(bytes))}</span>`;
    const clear = this.options.data || this.options.cache ? `<button class="text-button quiet-button data-clear" type="button" data-act="clear">${escape(words.clear)}</button>` : "";
    return `<section class="home-section data-section"><h2 class="section-title"><span title="${escape(words.cacheSaid)}">${escape(words.cache)}</span>${sized}${clear}</h2></section>`;
  }

  private async click(event: Event): Promise<void> {
    const target = event.target as HTMLElement;
    const group = target.closest<HTMLElement>("[data-group]")?.dataset.group as Group | undefined;
    if (group) {
      this.group = group;
      this.draw();
      return;
    }
    const open = target.closest<HTMLElement>("[data-open]")?.dataset.open;
    if (open) {
      if (this.open.has(open)) this.open.delete(open);
      else this.open.add(open);
      this.draw();
      return;
    }
    const days = target.closest<HTMLElement>("[data-days]")?.dataset.days;
    if (days && this.space && this.options.data) {
      try {
        const reply = await this.options.data({ do: "output_days", days: Number(days) });
        if (reply.data === "output_days") this.space.output_days = reply.days;
      } catch (error) {
        this.said = { text: this.options.words.failed, detail: detailOf(error) };
      }
      this.draw();
      return;
    }
    const act = target.closest<HTMLElement>("[data-act]")?.dataset.act;
    if (act === "delete") this.confirmDelete();
    else if (act === "clear" && (this.options.data || this.options.cache)) {
      try {
        if (this.options.data) await this.options.data({ do: "clear_cache" });
        await this.options.cache?.clear();
        this.said = { text: this.options.words.cleared, detail: "" };
      } catch (error) {
        this.said = { text: this.options.words.failed, detail: detailOf(error) };
      }
      await this.read();
    }
  }

  private confirmDelete(): void {
    const { words, confirm } = this.options;
    const chosen = (this.space?.conversations ?? []).filter((one) => this.chosen.has(one.name));
    if (!chosen.length) return;
    const guests = chosen.filter((one) => one.group === "elsewhere").length;
    confirm.ask({
      title: words.deleteTitle(chosen.length),
      what: words.deleteWhat(chosen.length - guests, guests),
      go: words.deleteGo,
      act: async () => {
        const reply = await this.options.data!({ do: "delete", names: chosen.map((one) => one.name) });
        if (reply.data !== "deleted") return;
        const done = reply.deletions.filter((one) => one.outcome.outcome === "deleted");
        const freed = done.reduce((sum, one) => sum + (one.outcome.outcome === "deleted" ? one.outcome.bytes : 0), 0);
        const refused = reply.deletions.filter((one) => one.outcome.outcome !== "deleted");
        this.said = {
          text: refused.length ? words.refused(refused[0]!.name, "why" in refused[0]!.outcome ? refused[0]!.outcome.why : "") : words.deleted(done.length, size(freed)),
          detail: refused.map((one) => `${one.name}: ${"why" in one.outcome ? one.outcome.why : ""}`).join("\n"),
        };
        this.chosen.clear();
      },
      done: () => void this.read(),
    });
  }
}

/** Everything World keeps on this computer. */
export function totalOf(space: Space): number {
  const { kinds } = space;
  return kinds.conversations + kinds.host_records + kinds.command_output + kinds.cache + kinds.program + kinds.other;
}

const size = (bytes: number) => bytesSaid(bytes);

export { bytesSaid };
