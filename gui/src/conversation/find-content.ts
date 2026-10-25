import type { Words } from "../words";
import type { Tool } from "../world/events";
import { isThinking, runsOf, startsAgent, type Block, type Entry } from "./model";
import { numbered, parse } from "./diff";
import { decodeRow, memoryDiff } from "./diff-lines";
import { stretchSays, thoughtOnly, thoughtText, type ProseEntry, type StretchAgents } from "./stretch-says";
import { plain, proseBlocks, render } from "./prose";
import { shownOutput } from "./run-output";
import { htmlText } from "./find-text";
import type { FindPart, FindTarget } from "./find-index";

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
  /** The rows a run of members is drawn as: a run of thinking is one. */
  rows(run: Entry[]): Row[];
  /** The tokens a run of thinking took, where the host said. */
  tokens(run: readonly ProseEntry[]): number | null;
  /** The host's own agents its calls start or name. */
  agents: StretchAgents;
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
    // Turns not held are found by the shell, not here.
    if (block.kind === "gap") return;
    if (block.kind === "say") {
      yield part("body", stamp, {}, () => htmlText(render(block.text)));
      return;
    }
    if (block.kind !== "activity-group") {
      yield part("body", stamp, {}, () => this.rendering.entry(block));
      if (block.kind === "heard" && this.rendering.notice(block)) {
        const notice = block.notice!;
        if (notice.kind === "answer" && notice.end.last_message) {
          yield part("answer", stamp, { row: block.key, selector: ".notice-answer" }, () => htmlText(render(notice.end.last_message!)));
        } else if (notice.kind === "world" && (notice.fact.fact === "moved" || notice.fact.fact === "focus")) {
          yield part("brief", stamp, { row: block.key, selector: ".told-plain" }, () => htmlText(plain(block.text)));
        }
      }
      const handed = block.kind === "agent-told" ? (block.end.answer ?? block.end.reason) : null;
      if (handed) yield part("answer", stamp, { row: block.key, selector: ".notice-answer" }, () => htmlText(render(handed)));
      return;
    }
    const said = stretchSays(block, words, this.rendering.attachment, this.rendering.tokens, this.rendering.agents);
    yield part("heading", `${stamp}|${block.members.length}|${said.time}`, { selector: ".group-head" }, () =>
      `${said.title}${said.added || said.removed ? `\n+${said.added} −${said.removed}` : ""}${said.time ? `\n${said.time}` : ""}`);
    // A stretch that is only thinking holds its words where its rows would be.
    if (thoughtOnly(block)) {
      const run = block.members as ProseEntry[];
      const key = `${run[0]!.key}~words`;
      yield part(`${key}/thinking`, `${stamp}|${block.members.length}`, { row: key }, () => this.prose(thoughtText(run), run.some((entry) => entry.writing)));
      return;
    }
    for (const run of runsOf(block.members)) {
      const member = run[0]!;
      for (const row of this.rendering.rows(run)) {
        const rowStamp = row.signature;
        yield part(`${row.key}/head`, rowStamp, { row: row.key }, () => row.text());
        if (row.key !== member.key) {
          yield { ...part(`${row.key}/diff`, rowStamp, { row: row.key, long: 0 }, (signal) => this.rendering.write(row.key, signal)), remote: true };
        } else if (isThinking(member)) {
          const thoughts = run as ProseEntry[];
          yield part(`${row.key}/thinking`, rowStamp, { row: row.key, selector: ".thinking-text" }, () => this.prose(thoughtText(thoughts), thoughts.some((entry) => entry.writing)));
        } else if (member.kind === "tool" && !startsAgent(member.tool)) {
          // What an agent did is its own conversation, read as its box
          // opens: its row's heading is what is found of it.
          if (member.tool.does.as === "edit") {
            let index = 0;
            for (const change of member.tool.does.changes) {
              const rows = parse(change.diff);
              const lines = memoryDiff(rows).lines;
              const numberedRows = numbered(rows);
              // Each change's card says its path in its head.
              yield part(`${row.key}/path/${index}`, rowStamp,
                { row: row.key, selector: `.diff-card:nth-of-type(${index + 1}) .diff-card-path` }, () => change.path);
              yield part(`${row.key}/diff/${index}`, rowStamp, { row: row.key, long: index }, () => {
                return lines.map((line) => displayedDiff(line, numberedRows, words.unchangedLines)).join("\n");
              });
              index += 1;
            }
          } else {
            const { given, shown, output, ending } = toolText(member.tool, words);
            let index = 0;
            if (given) yield part(`${row.key}/in`, rowStamp, { row: row.key, long: index++ }, () => given.replace(/\n$/, ""));
            if (shown || output) yield part(`${row.key}/out`, rowStamp, { row: row.key, long: index }, () => shown ? shown.lines.join("\n") : output.replace(/\n$/, ""));
            if (ending) yield part(`${row.key}/end`, rowStamp, { row: row.key, selector: ".io-end" }, () => ending);
          }
        }
      }
    }
  }
}
