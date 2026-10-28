// The codes that still let devices into a network (`world-network.md`,
// 入网码): one row each, what it is for drawn by its mark beside its name,
// how full it is as a short bar at its end, and a way to revoke it; how many came in
// by it, until when and where it was made are said when it is pointed at.
// The same rows on 「我」 and in the sheet that adds a device; the
// network's administrator sees every account's, each marked whose.

import { glyph, type GlyphName } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { lasting, type HubAccounts, type Login, type TokenSeen } from "../world/accounts";
import { row } from "./rows";
import { bar } from "./usage/draw";

/** How long a code's bar is drawn. */
const METER_WIDTH = 64;

export interface CodeWords {
  /** A code with no name, by how it is given. */
  ways: { scan: string; line: string; paste: string; managed: string; managedLine: string; other: string; ssh(name: string): string };
  /** What a named code lets in, by how it is given. */
  kinds: { scan: string; line: string; paste: string; managed: string };
  /** How many came in by it, of how many it lets in, any at zero. */
  came(used: number, uses: number): string;
  until(at: string): string;
  /** A code that lasts until it is revoked. */
  lasting: string;
  madeOn(machine: string): string;
  whose(account: string): string;
  revoke: string;
  revokeTitle(name: string): string;
  revokeWhat: string;
  revokeGo: string;
}

export const zhCodes: CodeWords = {
  ways: {
    scan: "手机二维码",
    line: "Linux 一行命令",
    paste: "电脑入网码",
    managed: "受管机器入网码",
    managedLine: "受管 Linux 一行命令",
    other: "入网码",
    ssh: (name) => `经 SSH 加 ${name}`,
  },
  kinds: { scan: "手机", line: "Linux 主机", paste: "电脑", managed: "受管机器" },
  came: (used, uses) => (uses === 0 ? `已进 ${used} 台，不限台数` : uses === 1 ? (used ? "已用" : "还没用") : `已进 ${used} / ${uses} 台`),
  until: (at) => `到 ${at}`,
  lasting: "不过期",
  madeOn: (machine) => `${machine} 生成`,
  whose: (account) => `${account} 的`,
  revoke: "作废",
  revokeTitle: (name) => `作废「${name}」？`,
  revokeWhat: "已经进来的设备照旧，新的进不来。",
  revokeGo: "作废",
};

export const enCodes: CodeWords = {
  ways: {
    scan: "Phone QR code",
    line: "Linux command",
    paste: "Computer code",
    managed: "Managed machine code",
    managedLine: "Managed Linux command",
    other: "Code",
    ssh: (name) => `${name} over SSH`,
  },
  kinds: { scan: "Phone", line: "Linux host", paste: "Computer", managed: "Managed" },
  came: (used, uses) => (uses === 0 ? `${used} in, any number` : uses === 1 ? (used ? "Used" : "Not used yet") : `${used} of ${uses} in`),
  until: (at) => `until ${at}`,
  lasting: "Never expires",
  madeOn: (machine) => `made on ${machine}`,
  whose: (account) => `${account}'s`,
  revoke: "Revoke",
  revokeTitle: (name) => `Revoke "${name}"?`,
  revokeWhat: "The devices already in stay; no new one comes in.",
  revokeGo: "Revoke",
};

/** How a code is given, as an earlier hub that does not say would have
 *  given it. */
function wayOf(token: TokenSeen): NonNullable<TokenSeen["way"]> {
  return token.way ?? (token.kind === "phone" ? "scan" : "paste");
}

/** What a code is called: its name, the one machine it names, or how it is
 *  given. */
export function codeName(token: TokenSeen, words: CodeWords): string {
  if (token.label) return token.label;
  const way = wayOf(token);
  if (way === "ssh") return words.ways.ssh(token.name ?? "");
  if (token.name) return token.name;
  if (way === "line") return token.managed ? words.ways.managedLine : words.ways.line;
  if (way === "scan") return words.ways.scan;
  if (way === "other") return words.ways.other;
  return token.managed ? words.ways.managed : words.ways.paste;
}

function codeMark(token: TokenSeen): GlyphName {
  const way = wayOf(token);
  if (way === "scan") return "phone";
  if (way === "line" || way === "ssh") return "terminal";
  return token.managed ? "shield" : "key";
}

/** The codes of `hub` this device is told of and may revoke as `login`:
 *  its account's first, then, for the administrator, the others'. A code
 *  past its day is gone, though the server tells of it only with its next
 *  change. */
export function codesOf(hub: HubAccounts | undefined, login: Login, now: Date): TokenSeen[] {
  const open = (hub?.tokens ?? []).filter((token) => new Date(token.expires) > now);
  return [...open.filter((token) => token.account === login.account), ...open.filter((token) => token.account !== login.account)];
}

/** One code's row, its revoking button marked with its key. */
export function codeRow(token: TokenSeen, login: Login, here: string, words: CodeWords, when: (at: Date) => string): string {
  const way = wayOf(token);
  const kind = token.managed ? words.kinds.managed : way === "scan" ? words.kinds.scan : way === "line" ? words.kinds.line : words.kinds.paste;
  const whose = token.account !== login.account ? words.whose(token.account) : "";
  const said = [
    whose,
    kind,
    words.came(token.used, token.uses),
    lasting(token.expires) ? words.lasting : words.until(when(new Date(token.expires))),
    token.made_by && token.made_by !== here ? words.madeOn(token.made_by) : "",
  ].filter(Boolean);
  // A code for one device or for any number has no bar to fill.
  const full = token.uses > 1 ? `<span class="code-meter">${bar(token.used / token.uses, `code:${token.key}`, METER_WIDTH, 10)}</span>` : "";
  return row(
    glyph(codeMark(token), 16),
    escape(codeName(token, words)),
    escape(whose),
    "",
    `${full}<button class="text-button quiet-button" type="button" data-act="revoke" data-key="${escape(token.key)}">${escape(words.revoke)}</button>`,
    // Its bar and its revoking stay on its line, even on a phone.
    " code-row act",
    said.join(" "),
  );
}
