import type { Words } from "../words";
import type { Tool } from "../world/events";
import { visible, withheld, type Activity, type Block, type Entry } from "./model";
import { counts, numbered, parse } from "./diff";
import { decodeRow, memoryDiff } from "./diff-lines";
import { groupHeading, headline } from "./tool";
import { plain, proseBlocks, render } from "./prose";
import { shownOutput } from "./run-output";
import { htmlText } from "./find-text";
import type { FindPart, FindTarget } from "./find-index";

type ToolEntry = Extract<Entry, { kind: "tool" }>;

/** A diff line's displayed fields, without syntax coloring or DOM work. */
export function displayedDiff(line: string, numbered: boolean, unchanged: (n: number) => string): string {
  const { row } = decodeRow(line);
  if (row.kind === "gap") return row.hidden === 0 ? "" : row.hidden ? `⋯ ${unchanged(row.hidden)}` : "⋯";
  const fields = [numbered && row.old !== null ? String(row.old) : "", numbered && row.new !== null ? String(row.new) : "",
    row.kind === "add" ? "+" : row.kind === "del" ? "−" : " ", row.text || " "];
  let text = "";
  for (const field of fields) {
    if (!field) continue;
    if (text && !/\s$/.test(text) && !/^\s/.test(field)) text += " ";
    text += field;
  }
  return text;
}

/** The same heading is used by the drawn stretch and its search text. */
export function stretchSays(group: Activity, words: Words, attachment: (path: string) => string | null) {
  const tools = group.members.filter((member): member is ToolEntry => member.kind === "tool");
  const thinking = group.members.filter((member) => member.kind === "prose");
  const steps = group.members.filter(visible).length;
  const writing = thinking.find((member) => member.kind === "prose" && member.writing);
  const runningTool = tools.find((tool) => tool.ended === null);
  const running = !!runningTool || !!writing;
  const bare = !running && withheld(group);
  let title: string;
  let added = 0;
  let removed = 0;
  if (bare) title = words.thoughtWithheld;
  else if (runningTool) {
    const line = headline(runningTool.tool, null, words, attachment);
    title = [line.action, line.object].filter(Boolean).join(" ");
  } else if (writing && writing.kind === "prose") {
    const titles = [...writing.text.matchAll(/^\*\*(.+?)\*\*\s*$/gm)].map((match) => match[1]);
    const last = titles[titles.length - 1];
    title = last ? `${words.thought}：${last}` : words.thinking;
  } else {
    title = groupHeading(tools, thinking.length, words);
    for (const tool of tools) {
      for (const write of tool.tool.run?.writes ?? []) {
        added += write.added ?? 0;
        removed += write.removed ?? 0;
      }
      if (tool.tool.does.as !== "edit") continue;
      for (const change of tool.tool.does.changes) {
        const counted = counts(change.diff);
        added += counted.added;
        removed += counted.removed;
      }
    }
  }
  let began: number | null = null;
  let last: number | null = null;
  for (const tool of tools) {
    if (began === null || tool.began < began) began = tool.began;
    if (tool.ended && (last === null || tool.ended.at > last)) last = tool.ended.at;
  }
  const ended = !running && tools.length && tools.every((tool) => tool.ended) ? last : null;
  const elapsed = running && began !== null ? Date.now() - began : began !== null && ended !== null ? ended - began : null;
  const time = [(running ? began !== null : steps >= 2) ? words.steps(steps) : "", elapsed !== null ? words.took(elapsed) : ""].filter(Boolean).join(" · ");
  return { title, added, removed, steps, running, bare, began, ended, time };
}

/** Input and output as the tool detail displays them, including localized
 * omissions and excluding write notes represented by their own rows. */
export function toolText(tool: Tool, words: Words) {
  const run = tool.does.as === "command" && tool.status !== "running" ? tool.run : undefined;
  const shown = run ? shownOutput(run.spans, run.writes, words.output) : null;
  const output = run ? "" : tool.output.filter((part) => part.type === "text").map((part) => part.text).join("\n");
  const given = tool.does.as === "command" ? tool.does.command
    : tool.input !== null && tool.input !== undefined && JSON.stringify(tool.input) !== "{}" ? JSON.stringify(tool.input, null, 2) : "";
  const ending = tool.does.as === "command" && tool.does.job ? words.wentOnAs(tool.does.job)
    : tool.does.as === "command" && tool.does.exit !== null ? words.exited(tool.does.exit) : "";
  return { run, shown, output, given, ending };
}

interface Row {
  key: string;
  signature: string;
  text(): string;
}

export interface FindRendering {
  words: Words;
  attachment(path: string): string | null;
  entry(entry: Entry): string;
  rows(entry: Entry): Row[];
  notice(entry: Extract<Entry, { kind: "heard" }>): boolean;
  /** Retained write changes are read through the view's existing source. */
  write(key: string, signal: AbortSignal): Promise<string>;
}

/** A projection of what can be read, without creating any live blocks,
 * rows, disclosure state or line viewers. */
export class FindContent {
  private proseParts = new Map<string, string>();
  constructor(private readonly rendering: FindRendering) {}

  prose(text: string, writing: boolean): string {
    return proseBlocks(text, writing).map((block) => {
      // Reference links depend on definitions elsewhere in this message.
      // Their source alone cannot identify their displayed text.
      if (block.source.includes("[")) return htmlText(block.html());
      let shown = this.proseParts.get(block.source);
      if (shown === undefined) {
        shown = htmlText(block.html());
        this.proseParts.set(block.source, shown);
      }
      return shown;
    }).join("");
  }

  *parts(block: Block): Iterable<FindPart> {
    const { words } = this.rendering;
    const part = (key: string, stamp: string, target: Omit<FindTarget, "block">, read: FindPart["read"]): FindPart =>
      ({ key: `${block.key}/${key}`, stamp, target: { block: block.key, ...target }, read });
    const stamp = String(block.stamp);
    const long = (key: string, row: string | undefined, index: number, text: string): FindPart =>
      part(key, stamp, { ...(row ? { row } : {}), long: index }, () => text.replace(/\n$/, ""));
    if (block.kind !== "activity-group") {
      yield part("body", stamp, {}, () => this.rendering.entry(block));
      if (block.kind === "heard" && this.rendering.notice(block)) {
        const notice = block.notice!;
        if (notice.kind === "job_ended" && notice.last?.text) {
          yield long("last", block.key, 0, notice.last.text);
          yield part("path", stamp, { row: block.key, selector: ".notice-path" }, () => `${notice.machine} · ${notice.output}`);
        } else if (notice.kind === "answer" && notice.end.last_message) {
          yield part("answer", stamp, { row: block.key, selector: ".notice-answer" }, () => htmlText(render(notice.end.last_message!)));
        } else if (notice.kind === "world" && notice.fact.fact === "moved") {
          yield part("brief", stamp, { row: block.key, selector: ".told-plain" }, () => htmlText(plain(block.text)));
        }
      }
      return;
    }
    const said = stretchSays(block, words, this.rendering.attachment);
    yield part("heading", `${stamp}|${block.members.length}|${said.time}`, { selector: ".group-head" }, () =>
      `${said.title}${said.added || said.removed ? `\n+${said.added} −${said.removed}` : ""}${said.time ? `\n${said.time}` : ""}`);
    for (const member of block.members.filter(visible)) {
      for (const row of this.rendering.rows(member)) {
        const rowStamp = row.signature;
        yield part(`${row.key}/head`, rowStamp, { row: row.key }, () => row.text());
        if (row.key !== member.key) {
          yield part(`${row.key}/diff`, rowStamp, { row: row.key, long: 0 }, (signal) => this.rendering.write(row.key, signal));
        } else if (member.kind === "prose") {
          yield part(`${row.key}/thinking`, rowStamp, { row: row.key, selector: ".thinking-text" }, () => this.prose(member.text, member.writing));
        } else if (member.kind === "tool") {
          if (member.tool.does.as === "edit") {
            let index = 0;
            for (const change of member.tool.does.changes) {
              const rows = parse(change.diff);
              const lines = memoryDiff(rows).lines;
              const numberedRows = numbered(rows);
              if (member.tool.does.changes.length > 1) yield part(`${row.key}/path/${index}`, rowStamp,
                { row: row.key, selector: `.diff-path:nth-of-type(${index + 1})` }, () => change.path);
              yield part(`${row.key}/diff/${index}`, rowStamp, { row: row.key, long: index }, () => {
                return lines.map((line) => displayedDiff(line, numberedRows, words.unchangedLines)).join("\n");
              });
              index += 1;
            }
          } else {
            const { given, shown, output, ending } = toolText(member.tool, words);
            let index = 0;
            if (given) {
              yield part(`${row.key}/in-label`, rowStamp, { row: row.key, selector: ".io-part:first-child > .io-label" }, () => "IN");
              yield part(`${row.key}/in`, rowStamp, { row: row.key, long: index++ }, () => given.replace(/\n$/, ""));
            }
            if (shown || output || ending || member.tool.output.some((one) => one.type === "image" || one.type === "image_url")) {
              yield part(`${row.key}/out-label`, rowStamp, { row: row.key, selector: `.io-part:nth-child(${given ? 2 : 1}) > .io-label` }, () => "OUT");
            }
            if (shown || output) yield part(`${row.key}/out`, rowStamp, { row: row.key, long: index }, () => shown ? shown.lines.join("\n") : output.replace(/\n$/, ""));
            if (ending) yield part(`${row.key}/end`, rowStamp, { row: row.key, selector: ".io-end" }, () => ending);
          }
        }
      }
    }
  }
}
