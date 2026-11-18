// The agents' first page, in its two views. 「额度」: the logins one works
// with, whichever machine each is on, under the agent each is a login of,
// with how full their windows are, in percent and in quota dollars; then
// the logins of others that assist me. 「接入」: one machine's agents and
// the logins found there, each line's plug saying whether Fastctx is
// connected to it.

import { reachDrawing } from "../app/agents-reach";
import type { Pickable } from "../app/agent-machines";
import { agentGlyph, glyph } from "../conversation/glyphs";
import { converses, daysOf, deeds, familyName, logins, machines, plateOf, usable, type Family, type Login, type Machine } from "./data";
import { whole } from "./format";
import { rule } from "./marks";
import { draw, goTo, linkOf, on, say, state } from "./state";
import { chevron, escape, gauge, machineOf, menu, toggle } from "./ui";

/** The part of a login's week Fable may use, of a hundred (用户
 *  2026-11-15): at it, the whole login stops Fable. */
export const FABLE_LINE = 50;
/** Whether the reader sees a login whole: its owner, or one it assists
 *  with no limit set. One given a limit sees their own part alone. */
export const seesWhole = (login: Login) => !login.lent || login.lent.limit === null;
/** Fable's part of a login's week, of a hundred: as the World says, or as
 *  its usage reckons it once fetched. */
export function fablePart(login: Login): number | undefined {
  if (login.fable !== undefined) return login.fable;
  if (login.family !== "claude" || !seesWhole(login)) return undefined;
  const days = usable(daysOf(login));
  if (!days) return undefined;
  return (days.people.reduce((sum, one) => sum + one.fable, 0) / plateOf(login)) * 100;
}
/** How much of what Fable may use of a login's week is used. */
export const fableUsed = (login: Login) => Math.min(100, Math.round(((fablePart(login) ?? 0) / FABLE_LINE) * 100));
export const fableSaid = "Fable 可用周额度的 50%，官方不提供读数，按账本推算。用尽后暂停 Fable，其他型号不受影响。";
/** So much of so much, in whole dollars. */
export const money = (percent: number, of: number) => `${whole((percent / 100) * of)} / ${whole(of)}`;
/** When a window starts over, as its bar says it. */
const startsOver = (reading: { resets: string; reset?: boolean }) => (reading.reset ? "已重置" : reading.resets ? `${reading.resets}重置` : "");

export function homePage(): string {
  const attach = state.place.kind === "attach";
  const machine = machines.find((one) => one.id === state.machine) ?? machines.find((one) => one.here) ?? machines[0];
  const end =
    attach && machine
      ? `<span class="lg-head-end">
        <button type="button" class="lg-pick" data-act="machines">${glyph(machine.owner ? "colleague" : machine.kind, 16)}<span>${machine.owner ? `${escape(machine.owner)} 的智能体` : escape(machine.id)}</span>${machine.here ? `<small>本机</small>` : ""}${chevron()}</button>
        ${machine.owner || !machine.online ? "" : `<button type="button" class="lg-word${machine.scanning ? " busy" : ""}" data-act="scan">${glyph("scan", 16)}${machine.scanning ? "正在扫描" : "重新扫描"}</button>`}
      </span>`
      : "";
  return `<div class="lg-page">
    <header class="page-head lg-head"><div class="page-head-title"><h1 class="page-title">智能体</h1></div>
      <div class="page-head-actions"><div class="segmented"><button type="button" class="segment${attach ? "" : " chosen"}" data-go="${linkOf({ kind: "quota" })}">额度</button><button type="button" class="segment${attach ? " chosen" : ""}" data-go="${linkOf({ kind: "attach", machine: machine?.id ?? state.machine })}">接入</button></div>${end}</div>
    </header>
    ${attach ? (machine ? attachBody(machine) : `<div class="lg-body lg-empty"><p>暂无机器</p></div>`) : quotaBody()}
  </div>`;
}

/** A login's own windows as its vendor reads them: how full each is, and
 *  when it starts over. Nothing here is reckoned. */
export function official(login: Login, tall?: number): [string, string] {
  if (login.unread && !login.five && !login.week) return [`<span class="lg-soft lg-unread" title="${escape(login.unread)}">未读取额度</span>`, ""];
  const idle = !machineOf(login).online;
  const more = tall ? { tall } : {};
  const drawn = (name: string, reading: Login["five"]) =>
    reading ? gauge(`${login.id}-${name}-${tall ?? 0}`, reading.reset ? 0 : reading.used, { ...more, idle, under: startsOver(reading), said: "官方读数" }) : "";
  return [drawn("five", login.five), drawn("week", login.week)];
}

/** What the ledger comes to of a login's week, of a hundred, where its
 *  usage has been fetched. */
export function weekPart(login: Login): number | undefined {
  const days = usable(daysOf(login));
  if (!days) return undefined;
  return (days.people.reduce((sum, one) => sum + one.week, 0) / plateOf(login)) * 100;
}

/** A login's week as the ledger reckons it: what was used of this week's
 *  figure, and of what Fable may use of it. */
export function reckoned(login: Login, tall?: number): [string, string] {
  const idle = !machineOf(login).online;
  const plate = plateOf(login);
  const more = tall ? { tall } : {};
  const week = weekPart(login);
  const fable = fablePart(login);
  return [
    week === undefined ? "" : gauge(`${login.id}-spent-${tall ?? 0}`, Math.round(week), { ...more, idle, under: money(week, plate), said: "按账本推算：本周额度美元 ÷ 本周周额度" }),
    fable === undefined ? "" : gauge(`${login.id}-fable-${tall ?? 0}`, fableUsed(login), { ...more, idle, near: fableUsed(login) >= 90, under: `${money(fableUsed(login), plate / 2)}${login.fableHeld ? `　<span class="lg-caution">已暂停</span>` : ""}`, said: fableSaid }),
  ];
}

/** A login's three columns on the page of limits: its two windows as the
 *  vendor reads them, then Fable as the ledger reckons it. */
export function windows(login: Login): string[] {
  const [five, week] = official(login);
  return [five, week, five.includes("lg-unread") ? "" : reckoned(login)[1]];
}

/** What I used this week of a login that assists me, in quota dollars: as
 *  its usage tells, which names me where it tells of others too. */
export function lentSpent(login: Login): { week: number; fable: number } {
  const days = usable(daysOf(login));
  const plate = plateOf(login);
  if (!days) return { week: ((login.lent?.used ?? 0) / 100) * plate, fable: ((login.lent?.fable ?? 0) / 100) * plate };
  const mine = days.people.some((one) => one.me) ? days.people.filter((one) => one.me) : days.people;
  return { week: mine.reduce((sum, one) => sum + one.week, 0), fable: mine.reduce((sum, one) => sum + one.fable, 0) };
}

/** What one given a limit sees of a login that assists them: their own
 *  part of its week and of its Fable, and nothing of the login's own. */
export function allotted(login: Login, tall?: number): string[] {
  const lent = login.lent!;
  const allot = (plateOf(login) * lent.limit!) / 100;
  const { week, fable: fableSpent } = lentSpent(login);
  const used = (week / allot) * 100;
  const fable = (fableSpent / (allot / 2)) * 100;
  const more = tall ? { tall } : {};
  const starts = login.week ? startsOver(login.week) : "";
  return [
    gauge(`${login.id}-mine-${tall ?? 0}`, Math.round(used), { ...more, near: used >= 85, under: `${money(used, allot)}${tall && starts ? `　${starts}` : ""}` }),
    login.family === "claude" ? gauge(`${login.id}-mine-fable-${tall ?? 0}`, Math.round(fable), { ...more, near: fable >= 85, under: money(fable, allot / 2), said: "Fable 上限为本人额度的一半" }) : "",
  ];
}

const loginPlace = (login: Login) => linkOf({ kind: "login", id: login.id, tab: "usage" });

function quotaBody(): string {
  // Each window says its own name too, for a narrow screen, where the
  // group's heads give way to rows stacked under the login.
  const cell = (drawn: string, head: string) => (drawn ? drawn.replace('<span class="lg-cell"', `<span class="lg-cell" data-head="${head}"`) : "<span></span>");
  const row = (login: Login, heads: [string, string, string]) => {
    const lent = login.lent;
    const cells = lent && lent.limit !== null ? ["", ...allotted(login)] : windows(login);
    const under = lent ? `来自 ${escape(lent.by)}　${lent.until ? `至 ${lent.until}` : "不限期"}` : `${escape(login.machine)}${login.plan ? `　${escape(login.plan)}` : ""}${machineOf(login).online ? "" : "　离线"}`;
    return `<div class="lg-row lg-cols${state.fresh === login.id ? " fresh" : ""}" role="link" tabindex="0" data-go="${loginPlace(login)}">
      <span class="lg-who"><b>${escape(login.who)}</b><small>${under}</small></span>
      ${cells.map((drawn, at) => cell(drawn, heads[at] ?? "")).join("")}
      <span class="lg-row-end"><button type="button" class="lg-word" data-act="new-talk" data-id="${escape(login.id)}">新对话</button></span>
    </div>`;
  };
  // Each group names its own columns on its head, right over its rows.
  const group = (name: string, mark: string, of: Login[], heads: [string, string, string], line: 1 | 2 | 3) =>
    of.length
      ? `<section class="lg-family"><div class="lg-family-head lg-cols"><h2>${mark}<span>${name}</span></h2>${heads.map((head) => `<span${head.startsWith("Fable") ? ` title="${fableSaid}"` : ""}>${head}</span>`).join("")}<span></span></div>${rule(line)}${of.map((login) => row(login, heads)).join("")}</section>`
      : "";
  const working = logins.filter((login) => login.workbench && !login.lent);
  const lent = logins.filter((login) => login.lent);
  if (!working.length && !lent.length) {
    const here = machines.find((one) => one.here) ?? machines[0];
    return `<div class="lg-body lg-empty"><span class="lg-empty-marks">${agentGlyph("claude", 34)}${agentGlyph("codex", 34)}</span><p>尚无开启工作台的登录</p>${here ? `<button type="button" class="lg-go" data-go="${linkOf({ kind: "attach", machine: here.id })}">接入</button>` : ""}</div>`;
  }
  return `<div class="lg-body lg-sheet"><div class="lg-quota">
    ${group(familyName("claude"), agentGlyph("claude", 20), working.filter((login) => login.family === "claude"), ["5 小时", "7 天", "Fable 推算"], 1)}
    ${group(familyName("codex"), agentGlyph("codex", 20), working.filter((login) => login.family === "codex"), ["5 小时", "7 天", ""], 2)}
    ${group("协助我的", glyph("colleague", 20), lent, ["5 小时", "7 天", "Fable 推算"], 3)}
  </div></div>`;
}

const pickable = (machine: Machine): Pickable => ({ name: machine.id, written: machine.id, here: machine.here, online: machine.online, since: null, owner: machine.owner, group: null, ownerLogin: null, kind: machine.kind }) as unknown as Pickable;

/** A folder's own name: the last of its path, or the drive it is. */
const tail = (dir: string) => dir.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || dir;

/** A login as a row of the machine's list: who it is, led by the plug that
 *  draws whether Fastctx is connected to it; where it is, said where its
 *  name does not say it; what is known of it; its workbench's switch; and
 *  the one deed on it. */
function rowOf(login: Login): string {
  const known: { text: string; said?: string; warn?: boolean }[] = [];
  if (login.plan) known.push({ text: login.plan });
  if (login.isDefault) known.push({ text: "默认" });
  if (login.token) known.push({ text: "长期令牌", said: login.planSet ? "以长期令牌登录，套餐由管理员设置" : "以长期令牌登录，无法识别邮箱与套餐" });
  if (login.loggedOut) known.push({ text: "未登录", warn: true });
  if (login.tooOld) known.push({ text: "版本过旧", said: login.tooOld, warn: true });
  if (login.lent) known.push({ text: `来自 ${login.lent.by}`, said: `协助${login.lent.until ? `至 ${login.lent.until}` : "不限期"}；任务在本人机器上运行` });
  // Found in a folder searched for it, the folder; known by its account,
  // the directory it is kept in; one named by its directory says no more.
  const where = login.under ? tail(login.under) : login.who.includes("@") && !login.isDefault ? tail(login.dir) : "";
  const id = escape(login.id);
  const connected = !login.busy && !login.lent && login.standing !== "found" && login.standing !== "renew";
  const bench = connected && converses(login.family) ? toggle("bench", login.workbench, "", `data-id="${id}" aria-label="工作台" title="在 Fastctx 中对话，记录用量与额度"`) : "";
  const act = login.busy
    ? `<span class="lg-soft">${escape(login.busy)}</span>`
    : login.lent
      ? `<button type="button" class="lg-word" data-act="new-talk" data-id="${id}">新对话</button>`
      : login.standing === "found"
        ? `<button type="button" class="lg-word lg-accent" data-act="attach" data-id="${id}">接入</button>`
        : login.standing === "renew"
          ? `<button type="button" class="lg-word lg-accent" data-act="attach" data-id="${id}" title="${escape(login.drift ?? "接入后其设置被修改")}">更新接入</button>`
          : converses(login.family)
            ? `<button type="button" class="lg-more" data-act="line-more" data-id="${id}" aria-label="更多">${glyph("more", 16)}</button>`
            : `<button type="button" class="lg-word" data-act="detach" data-id="${id}">移除</button>`;
  const who = login.workbench
    ? `<b class="lg-link" role="link" tabindex="0" data-go="${loginPlace(login)}" title="${escape(login.dir)}">${escape(login.who)}</b>`
    : `<b title="${escape(login.dir)}">${escape(login.who)}</b>`;
  return `<div class="lg-row lg-cols lg-still at-row${state.fresh === login.id ? " lg-lit" : ""}" data-standing="${login.standing}">
    <span class="at-who"><span class="at-plug" aria-hidden="true">${glyph("plug", 16)}</span>${who}</span>
    <span class="at-where"${login.under ? ` title="${escape(login.under)}"` : ""}>${escape(where)}</span>
    <span class="at-known">${known.map((one) => `<span${one.warn ? ` class="at-warn"` : ""}${one.said ? ` title="${escape(one.said)}"` : ""}>${escape(one.text)}</span>`).join("")}</span>
    <span class="at-bench">${bench}</span>
    <span class="at-act">${act}</span>
  </div>`;
}

const order: Family[] = ["claude", "codex", "antigravity", "cursor", "opencode"];
const ordered = (families: Family[]) => [...order.filter((family) => families.includes(family)), ...families.filter((family) => !order.includes(family)).sort()];

/** A machine's logins as one list, as the page of limits is one: a group
 *  for each agent under its own head, a row a login, every row on the same
 *  columns; then the folders searched for more, a row each. Cards of
 *  unlike heights side by side left gaps no one meant (the user,
 *  2026-11-16). */
function attachBody(machine: Machine): string {
  if (machine.silent) return `<div class="lg-body lg-empty">${glyph(machine.kind, 40)}<p>${escape(machine.id)} 无应答</p><p class="lg-soft">${escape(machine.silent)}</p></div>`;
  if (!machine.online) return `<div class="lg-body lg-empty">${glyph(machine.kind, 40)}<p>${escape(machine.id)} 离线</p></div>`;
  const here = logins.filter((login) => login.machine === machine.id);
  if (!machine.scanned && !here.length && !machine.owner)
    return `<div class="lg-body lg-empty">${glyph("scan", 40)}<p>尚未扫描${machine.here ? "本机" : ` ${escape(machine.id)}`}</p><button type="button" class="lg-go" data-act="scan">扫描</button></div>`;
  const families = ordered([...new Set(here.map((login) => login.family))]);
  // A family's logins: those connected first, then those found and not yet
  // connected, the ones a searched folder turned up last. Its head names
  // the columns its rows fill: the workbench only where its logins have one.
  const group = (family: Family, at: number) => {
    const rank = (login: Login) => (login.under ? 2 : login.standing === "found" ? 1 : 0);
    const of = here.filter((login) => login.family === family).sort((a, b) => rank(a) - rank(b));
    const heads = [of.some((login) => login.under || (login.who.includes("@") && !login.isDefault)) ? "位置" : "", of.some((login) => login.plan) ? "套餐" : "", converses(family) && !machine.owner ? "工作台" : ""];
    return `<section class="lg-family"><div class="lg-family-head lg-cols"><h2>${agentGlyph(family as never, 20)}<span>${escape(familyName(family))}</span></h2>${heads.map((head) => `<span>${head}</span>`).join("")}<span></span></div>${rule(((at % 3) + 1) as 1 | 2 | 3)}${of.map(rowOf).join("")}</section>`;
  };
  const groups = families.map(group).join("");
  if (machine.owner)
    return `<div class="lg-body lg-sheet"><div class="lg-attach">${groups}</div><p class="lg-note">${here.length ? `${escape(machine.owner)} 的登录正在协助你；任务在本人机器上运行。` : `${escape(machine.owner)} 的智能体未协助你。`}</p></div>`;
  // The folders searched for more logins, the last group: where each is,
  // how its search stands, and the one deed on it; the deed that adds one
  // stands at the group's head, over the rows' own.
  const changes = machine.here || !!deeds;
  const folderRow = (folder: NonNullable<Machine["folders"]>[number]) =>
    `<div class="lg-row lg-cols lg-still at-row at-folder">
      <span class="at-who"><span class="at-plug" aria-hidden="true">${glyph("folder", 16)}</span><b title="${escape(folder.dir)}"><bdi>${escape(folder.dir)}</bdi></b></span>
      <span class="at-said">${escape(folder.said)}</span>
      <span class="at-act">${changes ? `<button type="button" class="lg-word" data-act="folder" data-change="${folder.busy ? "stop" : "forget"}" data-dir="${escape(folder.dir)}">${folder.busy ? "停止" : "移除"}</button>` : ""}</span>
    </div>`;
  const folders = machine.folders
    ? `<section class="lg-family"><div class="lg-family-head lg-cols"><h2>${glyph("folder", 20)}<span>搜索的文件夹</span></h2><span></span><span></span><span></span><span class="at-act">${changes ? `<button type="button" class="lg-word" data-act="add-folder">${glyph("plus", 14)}添加</button>` : ""}</span></div>${rule(((families.length % 3) + 1) as 1 | 2 | 3)}${machine.folders.map(folderRow).join("")}${machine.folders.length ? "" : `<p class="lg-none at-none">尚未添加</p>`}</section>`
    : "";
  const others = machines.filter((one) => one !== machine && !one.owner);
  const reach = reachDrawing({ machine: pickable(machine), agents: families.map((family) => ({ agent: family, connected: here.some((login) => login.family === family && login.standing !== "found") })), others: others.map(pickable) } as never, 680, (more) => `另有 ${more} 台`);
  const missing = machine.missing ?? [];
  return `<div class="lg-body lg-sheet">
    <div class="ao-reach" title="${machine.here ? "本机" : "该机器"}已接入的智能体，可经 Fastctx 到达这些机器">${reach}</div>
    <div class="lg-attach">${groups}${folders}</div>
    ${missing.length ? `<p class="lg-note">${machine.here ? "本机" : "该机器"}未找到 ${missing.map(escape).join("、")}</p>` : ""}
    ${machine.scanFailed ? `<p class="lg-note">扫描未完成：${escape(machine.scanFailed)}</p>` : ""}
  </div>`;
}

on("machines", (anchor) => {
  const mine = machines.filter((machine) => !machine.owner);
  const lent = machines.filter((machine) => machine.owner);
  menu.open(
    anchor,
    "机器",
    [
      { note: "我的机器" },
      ...mine.map((machine) => ({ act: machine.id, icon: machine.kind, label: `${machine.id}${machine.here ? "　本机" : machine.online ? "" : "　离线"}`, checked: machine.id === state.machine })),
      ...(lent.length
        ? [{ rule: true as const }, { note: "协助我的" }, ...lent.map((machine) => ({ act: machine.id, icon: "colleague" as const, label: `${machine.owner} 的智能体`, checked: machine.id === state.machine }))]
        : []),
    ],
    (id) => {
      state.machine = id;
      goTo({ kind: "attach", machine: id });
    },
  );
});

on("scan", () => {
  const machine = machines.find((one) => one.id === state.machine);
  if (!machine || machine.scanning) return;
  if (deeds) return deeds.scan(machine);
  machine.scanning = true;
  draw();
  window.setTimeout(() => {
    machine.scanning = false;
    draw();
  }, 1800);
});

on("new-talk", (button) => {
  const login = logins.find((one) => one.id === button.dataset.id);
  if (!login) return;
  if (deeds) deeds.newTalk(login);
  else say(`新对话：${login.who}（样品未实现）`);
});
on("folder", (button) => {
  const machine = machines.find((one) => one.id === state.machine);
  if (deeds && machine) deeds.folder(machine, button.dataset.dir!, button.dataset.change as "stop" | "forget");
  else say(button.dataset.change === "stop" ? "已停止搜索（样品）" : "已移除文件夹（样品）");
});
on("add-folder", () => {
  const machine = machines.find((one) => one.id === state.machine);
  if (deeds && machine) deeds.addFolder(machine);
  else say("选择文件夹（样品未实现）");
});
