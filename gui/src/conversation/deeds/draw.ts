// What a run did, drawn under its row (`world-gui.md`, 对话): each deed a
// solid card as a change is, opening on a drawing of how it came out, its
// figures beside it, what is under it under them: tests a square each, red
// where they failed; a check a shield with its tick or its drops; a commit
// its seal and the way to the branch; a copy two machines, the sheet
// carried from one to the other; a job its terminal and how it ended, its
// last lines; a page published, a project entered, the plan written, a goal
// settled, a sentinel set; pictures as themselves. A command not
// recognised is named on a quiet line; at the foot, the run's own words,
// folded, as they were always shown.

import "./deeds.css";
import { glyph, type GlyphName } from "../glyphs";
import { drops } from "./marks";
import { carried, grid, pennant, planned, published, seal, shield, terminal, town, watching } from "../cards/drawings";
import { escape } from "../prose";
import type { LongText } from "../long-text";
import type { ChecksDeed, CommitDeed, CopyDeed, Deed, DeedWords, FileDeed, JobDeed, LogDeed, PageDeed, PictureDeed, PlainDeed, PushDeed, SearchDeed, StatusDeed, StopDeed, TestsDeed, WorldDeed, WritesDeed } from "./types";
import { bytesSaid } from "../../bytes";
import { fanCard, sentHtml, troubled } from "./fan";

export interface DeedTools {
  words: DeedWords;
  /** A file's lines in a card as a change's are: numbered, in its colours. */
  file(deed: FileDeed): { element: HTMLElement; longs: LongText[] };
  /** A picture the call gave, as an image's markup; empty where it has
   *  none to give. */
  picture(deed: PictureDeed): string;
}

export interface DrawnDeeds {
  element: HTMLElement;
  longs: LongText[];
}

/** Hits shown before the rest are asked for. */
const HITS = 24;

/** `deeds` drawn; `raw` is called the first time the run's own words are
 *  opened, and gives them; `ending` is how the run ended where that is
 *  worth saying. */
export function drawDeeds(deeds: readonly Deed[], tools: DeedTools, raw: () => HTMLElement, ending: string): DrawnDeeds {
  const element = document.createElement("div");
  element.className = "dd-deeds";
  const longs: LongText[] = [];
  deeds.forEach((deed, index) => {
    if (deed.kind === "writes") return;
    if (deed.kind === "file") {
      const drawn = tools.file(deed);
      longs.push(...drawn.longs);
      element.append(drawn.element);
      return;
    }
    const before = deeds[index - 1];
    if (deed.kind === "push" && before?.kind === "commit") return;
    // The pictures one command gave are one card.
    if (deed.kind === "picture") {
      if (before?.kind === "picture" && before.command === deed.command) return;
      const all: PictureDeed[] = [];
      for (let at = index; at < deeds.length; at += 1) {
        const one = deeds[at]!;
        if (one.kind !== "picture" || one.command !== deed.command) break;
        all.push(one);
      }
      element.append(pictureCard(all, tools));
      return;
    }
    const next = deeds[index + 1];
    if (deed.kind === "commit") {
      element.append(commit(deed, next?.kind === "push" ? next : null, tools.words));
      return;
    }
    element.append(drawn(deed, tools.words));
  });
  element.append(foot(tools.words, raw, ending));
  return { element, longs };
}

function drawn(deed: Exclude<Deed, FileDeed | WritesDeed | PictureDeed>, words: DeedWords): HTMLElement {
  switch (deed.kind) {
    case "plain":
      return plain(deed, words);
    case "fan":
      return fanCard(deed, words);
    case "tests":
      return tests(deed, words);
    case "checks":
      return checks(deed, words);
    case "commit":
      return commit(deed, null, words);
    case "status":
      return status(deed, words);
    case "log":
      return log(deed);
    case "push":
      return push(deed, words);
    case "search":
      return search(deed, words);
    case "copy":
      return copy(deed, words);
    case "job":
      return job(deed, words);
    case "stop":
      return stopped(deed, words);
    case "page":
      return page(deed, words);
    case "project":
      return scene(town(deed.town).svg, `<span class="dd-big">${escape(deed.town)}</span><span>${escape(words.entered)}</span>`);
    case "world":
      return settled(deed, words);
  }
}

/** A run that went on after its call came back: its terminal, still
 *  going, the job it goes on as. */
export function goingOn(job: string, machine: string | null, words: DeedWords): HTMLElement {
  return scene(terminal(job, "going").svg, `<span class="dd-big">${escape(words.inBackground)}</span><span class="dd-line"><code class="dd-hash">${escape(job)}</code>${machine ? `<span>${escape(machine)}</span>` : ""}</span>`);
}

/** A command named and nothing more; one an `on` carried, with what it
 *  last printed there and how it ended where it did not end well. */
function plain(deed: PlainDeed, words: DeedWords): HTMLElement {
  if (deed.on && (deed.on.tail || troubled(deed.on.end))) return html(`<div class="dd-card">${sentHtml(deed.command, deed.on.tail, deed.on.end, words)}</div>`);
  return html(`<div class="dd-plain">${glyph("command", 14)}<span>${escape(deed.command)}</span></div>`);
}

/** A copy: the two machines, the sheet carried between them; how much came
 *  across, what was there already, and what could not be copied, each as
 *  the World said why. */
function copy(deed: CopyDeed, words: DeedWords): HTMLElement {
  const from = [...new Set(deed.from.map((place) => place.machine))].join(" ");
  const said = [
    `<span class="dd-big">${escape(words.copied(deed.files, bytesSaid(deed.bytes)))}</span>`,
    `<span class="dd-line"><span>${escape(from)}</span><span class="dd-quiet">→</span><span>${escape(deed.to.machine)}</span><span class="dd-quiet dd-path-said" title="${escape(deed.to.path)}">${escape(deed.to.path)}</span></span>`,
    deed.same ? `<span class="dd-quiet">${escape(words.same(deed.same))}</span>` : "",
    deed.notCopied.length ? `<span class="dd-wrong">${escape(words.notCopied(deed.notCopied.length))}</span>` : "",
  ].join("");
  const body = deed.notCopied.length ? `<ul class="dd-list">${deed.notCopied.map((why) => `<li class="dd-ref wrong">${drops(why, 1, "wrong")}<span class="dd-why">${escape(why)}</span></li>`).join("")}</ul>` : "";
  return scene(carried(`${deed.from.map((place) => place.path).join("")}${deed.to.path}`, deed.notCopied.length > 0).svg, said, body);
}

/** A job that ended: its terminal, how it ended and where, how long ago;
 *  its last lines under. */
function job(deed: JobDeed, words: DeedWords): HTMLElement {
  // How long before the wait came back it ended.
  const before = deed.waited ? Date.parse(deed.waited) - Date.parse(deed.at) : NaN;
  const said = [
    `<span class="dd-big${deed.failed ? " dd-wrong" : ""}">${escape(words.ended(deed.end))}</span>`,
    `<span class="dd-line"><code class="dd-hash">${escape(deed.job)}</code><span>${escape(deed.machine)}</span>${before > 60_000 ? `<span class="dd-quiet">${escape(words.ago(before))}</span>` : ""}</span>`,
  ].join("");
  const body = deed.tail.length ? `<pre class="dd-said-text">${escape(deed.tail.join("\n"))}</pre>` : "";
  return scene(terminal(deed.job, deed.failed ? (deed.end.kind === "stopped" ? "stopped" : "failed") : "done").svg, said, body);
}

function stopped(deed: StopDeed, words: DeedWords): HTMLElement {
  const said = `<span class="dd-big${deed.stopped ? "" : " dd-wrong"}">${escape(words.stopped(deed.how))}</span><span class="dd-line"><code class="dd-hash">${escape(deed.what)}</code>${deed.machine ? `<span>${escape(deed.machine)}</span>` : ""}</span>`;
  return scene(terminal(deed.what, deed.stopped ? "stopped" : "failed").svg, said);
}

function page(deed: PageDeed, words: DeedWords): HTMLElement {
  const said = `<span class="dd-big">${escape(deed.title)}</span><span class="dd-line"><span>${escape(deed.name)}</span><span class="dd-quiet">${escape(words.version(deed.version))}</span></span>`;
  return scene(published(deed.name).svg, said);
}

/** The plan written, a goal settled, a sentinel set. */
function settled(deed: WorldDeed, words: DeedWords): HTMLElement {
  switch (deed.act) {
    case "plan":
      return scene(planned(deed.command ?? "plan").svg, `<span class="dd-big">${escape(words.world.plan)} ${escape(words.thePlan)}</span>`);
    case "sense":
      return scene(watching(deed.command ?? "sense").svg, `<span class="dd-big">${escape(deed.said ?? "")}</span><span class="dd-quiet">${escape(words.watching)}</span>`);
    default: {
      const state = deed.act === "goal-complete" ? "complete" : deed.act === "goal-blocked" ? "blocked" : "pause";
      return scene(pennant(deed.command ?? "goal", state).svg, `<span class="dd-big${state === "blocked" ? " dd-wrong" : ""}">${escape(words.goal[state].said)}</span>`);
    }
  }
}

/** Pictures a `view` gave, as themselves, each named under it. */
function pictureCard(deeds: readonly PictureDeed[], tools: DeedTools): HTMLElement {
  const { words } = tools;
  const shown = deeds
    .map((deed) => {
      const image = tools.picture(deed);
      const name = deed.page !== null ? words.page(deed.page) : leaf(deed.path);
      return `<figure class="dd-picture">${image || `<span class="dd-picture-none"></span>`}<figcaption><span>${escape(name)}</span><span class="dd-quiet">${deed.width}×${deed.height}</span></figcaption></figure>`;
    })
    .join("");
  return html(`<div class="dd-card dd-pictures">${shown}</div>`);
}

function card(mark: GlyphName, tone: string, title: string, figures: string, body = ""): HTMLElement {
  return html(
    `<div class="dd-card"><div class="dd-head"><span class="dd-mark${tone ? ` ${tone}` : ""}">${glyph(mark, 15)}</span><span class="dd-title">${title}</span>${figures ? `<span class="dd-figures">${figures}</span>` : ""}</div>${body ? `<div class="dd-body">${body}</div>` : ""}</div>`,
  );
}

/** A card whose head is a drawing of how it came out, its figures beside
 *  it, what it holds under them. */
function scene(art: string, said: string, body = ""): HTMLElement {
  return html(`<div class="dd-card dd-drawn"><div class="dd-scene"><span class="dd-art">${art}</span><span class="dd-said">${said}</span></div>${body ? `<div class="dd-body">${body}</div>` : ""}</div>`);
}

function tests(deed: TestsDeed, words: DeedWords): HTMLElement {
  const rows = deed.failures
    .map((failure) => opener(`${drops(failure.name, 1, "wrong")}<code class="dd-name">${escape(failure.name)}</code>`, failure.said.length ? `<pre class="dd-said-text">${escape(failure.said.join("\n"))}</pre>` : "", "dd-fail"))
    .join("");
  const said = [
    deed.failed ? `<span class="dd-big dd-wrong">${escape(words.failed(deed.failed))}</span>` : `<span class="dd-big">${escape(words.passed(deed.passed))}</span>`,
    deed.failed ? `<span>${escape(words.passed(deed.passed))}</span>` : "",
    deed.ignored ? `<span class="dd-quiet">${escape(words.skippedTests(deed.ignored))}</span>` : "",
  ].join("");
  const element = scene(grid(`${deed.command}`, deed), said, rows);
  wire(element);
  return element;
}

function checks(deed: ChecksDeed, words: DeedWords): HTMLElement {
  const errors = deed.items.filter((item) => item.level === "error").length;
  const warnings = deed.items.length - errors;
  const said = deed.clean || !deed.items.length
    ? `<span class="dd-big">${escape(words.clean)}</span>`
    : [errors ? `<span class="dd-big dd-wrong">${escape(words.errors(errors))}</span>` : "", warnings ? `<span class="${errors ? "" : "dd-big "}dd-caution">${escape(words.warnings(warnings))}</span>` : ""].join("");
  const rows = deed.items
    .map((item) => {
      const place = item.place ? `<span class="dd-place" title="${escape(`${item.place.path}:${item.place.line}:${item.place.column}`)}">${escape(`${leaf(item.place.path)}:${item.place.line}`)}</span>` : "";
      return opener(`${drops(`${item.message}${item.place?.line ?? ""}`, 1, item.level === "error" ? "wrong" : "caution")}<span class="dd-message">${escape(item.message)}</span>${place}`, item.snippet.length ? `<pre class="dd-said-text">${escape(item.snippet.join("\n"))}</pre>` : "", "dd-diag");
    })
    .join("");
  const element = scene(shield(`${deed.command}`, deed).svg, said, rows);
  wire(element);
  return element;
}

/** A commit: its seal, what it says, how much it changed; sent, where to,
 *  or that it was turned away. */
function commit(deed: CommitDeed, push: PushDeed | null, words: DeedWords): HTMLElement {
  const stats = deed.added !== null && deed.removed !== null ? `<span class="dd-stats"><span class="add">+${deed.added}</span><span class="del">−${deed.removed}</span></span>` : "";
  const files = deed.files !== null ? `<span>${escape(words.files(deed.files))}</span>` : "";
  const refused = push?.refs.some((ref) => !ref.ok) ?? false;
  const branches = push ? [...new Set(push.refs.map((ref) => ref.to))].join(" ") : "";
  const sent = push ? (refused ? `<span class="dd-wrong">${escape(words.rejected)}</span>` : push.current ? `<span>${escape(words.upToDate)}</span>` : branches ? `<span>${escape(words.pushedTo(branches))}</span>` : "") : "";
  const said = `<span class="dd-big dd-subject-line">${escape(deed.subject)}</span><span class="dd-line"><code class="dd-hash">${escape(deed.hash)}</code>${files}${stats}</span>${sent ? `<span class="dd-line">${sent}</span>` : ""}`;
  const why = push?.refs.filter((ref) => !ref.ok && ref.why).map((ref) => `<p class="dd-none">${escape(ref.why!)}</p>`).join("") ?? "";
  return scene(seal(deed.hash, [deed], push ? [push] : []).svg, said, why);
}

function status(deed: StatusDeed, words: DeedWords): HTMLElement {
  const rows = deed.files.length
    ? `<ul class="dd-list">${deed.files.map((file) => `<li class="dd-file"><span class="dd-state ${file.state}">${escape(words.states[file.state])}</span>${pathHtml(file.path)}</li>`).join("")}</ul>`
    : `<p class="dd-none">${escape(words.noChanges)}</p>`;
  return card("rows", "", `${escape(deed.command ?? "git status")}${deed.branch ? ` <span class="dd-quiet">${escape(deed.branch)}</span>` : ""}`, deed.files.length ? `<span>${escape(words.files(deed.files.length))}</span>` : "", rows);
}

function log(deed: LogDeed): HTMLElement {
  const rows = `<ul class="dd-list">${deed.commits.map((one) => `<li class="dd-commit"><code class="dd-hash">${escape(one.hash)}</code><span class="dd-subject">${escape(one.subject)}</span></li>`).join("")}</ul>`;
  return card("history", "", escape(deed.command ?? "git log"), "", rows);
}

function push(deed: PushDeed, words: DeedWords): HTMLElement {
  const bad = deed.refs.some((ref) => !ref.ok);
  const branches = [...new Set(deed.refs.map((ref) => ref.to))].join(" ");
  const said = bad
    ? `<span class="dd-big dd-wrong">${escape(words.rejected)}</span>`
    : deed.current
      ? `<span class="dd-big">${escape(words.upToDate)}</span>`
      : `<span class="dd-big">${escape(words.pushedTo(branches))}</span>${deed.remote ? `<span class="dd-quiet">${escape(deed.remote)}</span>` : ""}`;
  const why = deed.refs.filter((ref) => !ref.ok && ref.why).map((ref) => `<p class="dd-none">${escape(ref.why!)}</p>`).join("");
  return scene(seal(deed.command ?? "push", [], [deed]).svg, said, why);
}

function search(deed: SearchDeed, words: DeedWords): HTMLElement {
  const count = deed.files.reduce((sum, file) => sum + file.hits.length, 0);
  const pattern = matcher(deed.pattern, deed.ignoreCase);
  let shown = 0;
  const groups: string[] = [];
  const later: string[] = [];
  for (const file of deed.files) {
    const hits = file.hits.map(
      (hit) => `<li class="dd-hit"><span class="dd-line">${hit.line ?? ""}</span><code>${marked(hit.text, pattern)}</code></li>`,
    );
    const group = `<li class="dd-group">${pathHtml(file.path)}<ul class="dd-hits">${hits.join("")}</ul></li>`;
    (shown < HITS ? groups : later).push(group);
    shown += file.hits.length;
  }
  const more = later.length ? `<button class="dd-more" type="button">${escape(words.more(count - deed.files.slice(0, groups.length).reduce((sum, file) => sum + file.hits.length, 0)))}</button>` : "";
  const element = card("search", "", `<code class="dd-pattern">${escape(deed.pattern ?? "")}</code>`, `<span>${escape(words.hits(count, deed.files.length))}</span>`, `<ul class="dd-groups">${groups.join("")}</ul>${more}`);
  element.querySelector(".dd-more")?.addEventListener("click", (event) => {
    (event.currentTarget as HTMLElement).replaceWith(html(`<ul class="dd-groups">${later.join("")}</ul>`));
  });
  return element;
}

/** A row that opens what it holds under it. */
function opener(line: string, held: string, kind: string): string {
  if (!held) return `<div class="dd-opener ${kind}"><div class="dd-opener-line">${line}</div></div>`;
  return `<div class="dd-opener ${kind}"><button class="dd-opener-line" type="button" aria-expanded="false">${line}<span class="dd-chevron">${glyph("chevron", 14)}</span></button><div class="fold"><div class="fold-inner">${held}</div></div></div>`;
}

function wire(element: HTMLElement): void {
  for (const button of element.querySelectorAll<HTMLButtonElement>("button.dd-opener-line")) {
    button.addEventListener("click", () => {
      const open = button.getAttribute("aria-expanded") !== "true";
      button.setAttribute("aria-expanded", String(open));
      button.nextElementSibling?.classList.toggle("open", open);
    });
  }
}

/** The run's own words under what was drawn of them, opened on asking. */
function foot(words: DeedWords, raw: () => HTMLElement, ending: string): HTMLElement {
  const element = html(
    `<div class="dd-foot"><div class="dd-foot-line"><button class="dd-raw" type="button" aria-expanded="false">${escape(words.raw)}<span class="dd-chevron">${glyph("chevron", 14)}</span></button>${ending}</div><div class="fold"><div class="fold-inner"></div></div></div>`,
  );
  const button = element.querySelector<HTMLButtonElement>(".dd-raw")!;
  const fold = element.querySelector<HTMLElement>(".fold")!;
  const inner = fold.querySelector<HTMLElement>(".fold-inner")!;
  button.addEventListener("click", () => {
    const open = button.getAttribute("aria-expanded") !== "true";
    if (open && !inner.childElementCount) inner.append(raw());
    button.setAttribute("aria-expanded", String(open));
    fold.classList.toggle("open", open);
  });
  return element;
}

function pathHtml(path: string): string {
  const cut = path.replace(/\\/g, "/").lastIndexOf("/");
  const folder = cut < 0 ? "" : path.slice(0, cut + 1);
  return `<span class="dd-path" title="${escape(path)}"><span class="dd-folder"><bdi>${escape(folder)}</bdi></span><span class="dd-leaf">${escape(cut < 0 ? path : path.slice(cut + 1))}</span></span>`;
}

function leaf(path: string): string {
  const cut = path.replace(/\\/g, "/").lastIndexOf("/");
  return cut < 0 ? path : path.slice(cut + 1);
}

function matcher(pattern: string | null, ignoreCase: boolean): RegExp | null {
  if (!pattern) return null;
  try {
    return new RegExp(pattern, ignoreCase ? "gi" : "g");
  } catch {
    return new RegExp(pattern.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), ignoreCase ? "gi" : "g");
  }
}

/** A hit's line, what was looked for marked in it. */
function marked(text: string, pattern: RegExp | null): string {
  if (!pattern) return escape(text);
  let out = "";
  let at = 0;
  for (const match of text.matchAll(pattern)) {
    if (!match[0]) break;
    out += `${escape(text.slice(at, match.index))}<mark>${escape(match[0])}</mark>`;
    at = match.index! + match[0].length;
  }
  return out + escape(text.slice(at));
}

function html(markup: string): HTMLElement {
  const holder = document.createElement("div");
  holder.innerHTML = markup.trim();
  return holder.firstElementChild as HTMLElement;
}
