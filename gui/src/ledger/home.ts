// The agents' first page, in its two views. 「额度」: the logins one works
// with, whichever machine each is on, under the agent each is a login of,
// with how full their windows are, in percent and in quota dollars; then
// the logins of others that assist me. 「接入」: one machine's agents and
// the logins found there, each line's plug saying whether Fastctx is
// connected to it.

import { reachDrawing } from "../app/agents-reach";
import type { Pickable } from "../app/agent-machines";
import { loginLine, type LineTag } from "../app/login-line";
import { agentGlyph, glyph } from "../conversation/glyphs";
import { converses, daysOf, deeds, familyName, logins, machines, plateOf, usable, type Family, type Login, type Machine } from "./data";
import { whole } from "./format";
import { rule } from "./marks";
import { draw, goTo, linkOf, on, say, state } from "./state";
import { chevron, escape, gauge, machineOf, menu, toggle } from "./ui";

/** The part of a login's week Fable may use, of a hundred (用户
 *  2026-11-15): at it, the whole login stops Fable. */
export const FABLE_LINE = 50;
/** Fable's part of a login's week, of a hundred: as the World says, or as
 *  its usage reckons it once fetched. */
export function fablePart(login: Login): number | undefined {
  if (login.fable !== undefined) return login.fable;
  if (login.family !== "claude" || login.lent) return undefined;
  const days = usable(daysOf(login));
  if (!days) return undefined;
  return (days.people.reduce((sum, one) => sum + one.fable, 0) / plateOf(login)) * 100;
}
/** How much of what Fable may use of a login's week is used. */
export const fableUsed = (login: Login) => Math.min(100, Math.round(((fablePart(login) ?? 0) / FABLE_LINE) * 100));
export const fableSaid = "Fable 额度为周额度的 50%，官方读数不提供，按账本估算。用尽后暂停 Fable，其他型号不受影响。";
/** So much of so much, in whole dollars. */
export const money = (percent: number, of: number) => `${whole((percent / 100) * of)} / ${whole(of)}`;

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
    <header class="lg-head"><h1 class="lg-title">智能体</h1>
      <div class="segmented"><button type="button" class="segment${attach ? "" : " chosen"}" data-go="${linkOf({ kind: "quota" })}">额度</button><button type="button" class="segment${attach ? " chosen" : ""}" data-go="${linkOf({ kind: "attach", machine: machine?.id ?? state.machine })}">接入</button></div>${end}
    </header>
    ${attach ? (machine ? attachBody(machine) : `<div class="lg-body lg-empty"><p>暂无机器</p></div>`) : quotaBody()}
  </div>`;
}

/** A login's three windows as its owner sees them, and as one it assists
 *  without a limit does. */
export function windows(login: Login, tall?: number): string[] {
  if (login.unread && !login.five && !login.week) return [`<span class="lg-soft lg-unread" title="${escape(login.unread)}">未读取额度</span>`, "", ""];
  const idle = !machineOf(login).online;
  const plate = plateOf(login);
  const more = tall ? { tall } : {};
  const days = usable(daysOf(login));
  const fiveSpent = days?.fiveSpent ?? null;
  const five = login.five;
  const fable = fablePart(login);
  const fiveUnder = !five ? "" : five.reset ? "已重置" : `${fiveSpent !== null && five.used > 0 ? `${whole(fiveSpent)} / ${whole((fiveSpent * 100) / five.used)}` : `${five.used}%`}${five.resets ? `　${five.resets}${tall ? "重置" : ""}` : ""}`;
  return [
    five ? gauge(`${login.id}-five-${tall ?? 0}`, five.reset ? 0 : five.used, { ...more, idle, under: fiveUnder, said: `${five.resets}重置` }) : "",
    login.week ? gauge(`${login.id}-week-${tall ?? 0}`, login.week.reset ? 0 : login.week.used, { ...more, idle, under: login.week.reset ? "已重置" : `${money(login.week.used, plate)}${login.week.resets ? `　${login.week.resets}${tall ? " 重置" : ""}` : ""}`, said: `${login.week.resets} 重置` }) : "",
    fable === undefined ? "" : gauge(`${login.id}-fable-${tall ?? 0}`, fableUsed(login), { ...more, idle, near: fableUsed(login) >= 90, under: `${money(fableUsed(login), plate / 2)}${login.fableHeld ? `　<span class="lg-caution">已暂停</span>` : ""}`, said: fableSaid }),
  ];
}

/** What I used this week of a login that assists me, in quota dollars: as
 *  its usage tells, which to me is only mine. */
export function lentSpent(login: Login): { week: number; fable: number } {
  const days = usable(daysOf(login));
  const plate = plateOf(login);
  if (!days) return { week: ((login.lent?.used ?? 0) / 100) * plate, fable: ((login.lent?.fable ?? 0) / 100) * plate };
  return { week: days.people.reduce((sum, one) => sum + one.week, 0), fable: days.people.reduce((sum, one) => sum + one.fable, 0) };
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
  return [
    gauge(`${login.id}-mine-${tall ?? 0}`, Math.round(used), { ...more, near: used >= 85, under: money(used, allot) }),
    login.family === "claude" ? gauge(`${login.id}-mine-fable-${tall ?? 0}`, Math.round(fable), { ...more, near: fable >= 85, under: money(fable, allot / 2), said: "Fable 上限为本人额度的一半" }) : "",
  ];
}

const loginPlace = (login: Login) => linkOf({ kind: "login", id: login.id, tab: "usage" });

function quotaBody(): string {
  const cell = (drawn: string) => drawn || "<span></span>";
  const row = (login: Login) => {
    const lent = login.lent;
    const cells = lent && lent.limit !== null ? ["", ...allotted(login)] : windows(login);
    const under = lent ? `来自 ${escape(lent.by)}　${lent.until ? `至 ${lent.until}` : "不限期"}` : `${escape(login.machine)}${login.plan ? `　${escape(login.plan)}` : ""}${machineOf(login).online ? "" : "　离线"}`;
    return `<div class="lg-row lg-cols${state.fresh === login.id ? " fresh" : ""}" role="link" tabindex="0" data-go="${loginPlace(login)}">
      <span class="lg-who"><b>${escape(login.who)}</b><small>${under}</small></span>
      ${cells.map(cell).join("")}
      <span class="lg-row-end"><button type="button" class="lg-word" data-act="new-talk" data-id="${escape(login.id)}">新对话</button></span>
    </div>`;
  };
  // Each group names its own columns on its head, right over its rows.
  const group = (name: string, mark: string, of: Login[], heads: [string, string, string], line: 1 | 2 | 3) =>
    of.length
      ? `<section class="lg-family"><div class="lg-family-head lg-cols"><h2>${mark}<span>${name}</span></h2>${heads.map((head) => `<span${head === "Fable" ? ` title="${fableSaid}"` : ""}>${head}</span>`).join("")}<span></span></div>${rule(line)}${of.map(row).join("")}</section>`
      : "";
  const working = logins.filter((login) => login.workbench && !login.lent);
  const lent = logins.filter((login) => login.lent);
  if (!working.length && !lent.length) {
    const here = machines.find((one) => one.here) ?? machines[0];
    return `<div class="lg-body lg-empty"><span class="lg-empty-marks">${agentGlyph("claude", 34)}${agentGlyph("codex", 34)}</span><p>尚无开启工作台的登录</p>${here ? `<button type="button" class="lg-go" data-go="${linkOf({ kind: "attach", machine: here.id })}">接入</button>` : ""}</div>`;
  }
  return `<div class="lg-body lg-sheet"><div class="lg-quota">
    ${group(familyName("claude"), agentGlyph("claude", 20), working.filter((login) => login.family === "claude"), ["5 小时", "7 天", "Fable"], 1)}
    ${group(familyName("codex"), agentGlyph("codex", 20), working.filter((login) => login.family === "codex"), ["5 小时", "7 天", ""], 2)}
    ${group("协助我的", glyph("colleague", 20), lent, ["5 小时", "7 天", "Fable"], 3)}
  </div></div>`;
}

const pickable = (machine: Machine): Pickable => ({ name: machine.id, written: machine.id, here: machine.here, online: machine.online, since: null, owner: machine.owner, group: null, ownerLogin: null, kind: machine.kind }) as unknown as Pickable;

function lineOf(login: Login): string {
  const tags: LineTag[] = [];
  if (login.who.includes("@") && !login.isDefault) tags.push({ text: login.dir.split(/[\\/]/).pop()! });
  if (login.plan) tags.push({ text: login.plan });
  if (login.isDefault) tags.push({ text: "默认" });
  if (login.token) tags.push({ text: "长期令牌", said: login.planSet ? "以长期令牌登录，套餐由管理员设置" : "以长期令牌登录，无法识别邮箱与套餐" });
  if (login.loggedOut) tags.push({ text: "未登录", warn: true } as LineTag);
  if (login.tooOld) tags.push({ text: "版本过旧", said: login.tooOld, warn: true } as LineTag);
  if (login.lent) tags.push({ text: `来自 ${login.lent.by}`, said: `协助${login.lent.until ? `至 ${login.lent.until}` : "不限期"}；任务在本人机器上运行` });
  const id = escape(login.id);
  const act = login.busy
    ? `<span class="lg-soft">${escape(login.busy)}</span>`
    : login.lent
      ? `<button type="button" class="text-button ll-quiet" data-act="new-talk" data-id="${id}">新对话</button>`
      : login.standing === "found"
        ? `<button type="button" class="text-button ll-go" data-act="attach" data-id="${id}">接入</button>`
        : login.standing === "renew"
          ? `<button type="button" class="text-button ll-go" data-act="attach" data-id="${id}" title="${escape(login.drift ?? "接入后其设置被修改")}">更新接入</button>`
          : converses(login.family)
            ? `${toggle("bench", login.workbench, "工作台", `data-id="${id}" title="在 Fastctx 中对话，记录用量与额度"`)}<button type="button" class="lg-more" data-act="line-more" data-id="${id}" aria-label="更多">${glyph("more", 16)}</button>`
            : `<button type="button" class="text-button ll-quiet" data-act="detach" data-id="${id}">移除</button>`;
  const line = loginLine({ standing: login.standing, who: login.who, whoSaid: login.dir, tags, act, ...(state.fresh === login.id ? { fresh: ` lg-lit"` } : {}) });
  return login.workbench ? line.replace(`<span class="ll-who"`, `<span class="ll-who lg-link" role="link" tabindex="0" data-go="${loginPlace(login)}"`) : line;
}

const order: Family[] = ["claude", "codex", "antigravity", "cursor", "opencode"];
const ordered = (families: Family[]) => [...order.filter((family) => families.includes(family)), ...families.filter((family) => !order.includes(family)).sort()];

function attachBody(machine: Machine): string {
  if (machine.silent) return `<div class="lg-body lg-empty">${glyph(machine.kind, 40)}<p>${escape(machine.id)} 无应答</p><p class="lg-soft">${escape(machine.silent)}</p></div>`;
  if (!machine.online) return `<div class="lg-body lg-empty">${glyph(machine.kind, 40)}<p>${escape(machine.id)} 离线</p></div>`;
  const here = logins.filter((login) => login.machine === machine.id);
  if (!machine.scanned && !here.length && !machine.owner)
    return `<div class="lg-body lg-empty">${glyph("scan", 40)}<p>尚未扫描${machine.here ? "本机" : escape(machine.id)}</p><button type="button" class="lg-go" data-act="scan">扫描</button></div>`;
  const families = ordered([...new Set(here.map((login) => login.family))]);
  const card = (family: Family, of: Login[]) => `<section class="ao-card"><header class="ao-head"><span class="ao-mark">${agentGlyph(family as never, 22)}</span><h2 class="ao-name">${escape(familyName(family))}</h2></header><ul class="ao-logins">${of.map(lineOf).join("")}</ul></section>`;
  if (machine.owner)
    return `<div class="lg-body lg-sheet"><div class="ao-cards">${families.map((family) => card(family, here.filter((login) => login.family === family))).join("")}</div><p class="lg-note">${here.length ? `${escape(machine.owner)} 的登录正在协助你；任务在本人机器上运行。` : `${escape(machine.owner)} 的智能体未协助你。`}</p></div>`;
  const cards = families.map((family) => card(family, here.filter((login) => login.family === family && !login.under))).join("");
  const found = here.filter((login) => login.under);
  const folders = machine.folders
    ? `<section class="ao-card lg-folders"><header class="ao-head"><span class="ao-mark">${glyph("folder", 22)}</span><h2 class="ao-name">搜索的文件夹</h2>${machine.here ? `<button type="button" class="text-button ll-go" data-act="add-folder">${glyph("plus", 14)}添加文件夹…</button>` : ""}</header>
        <ul class="ao-logins">${machine.folders
          .map((folder) => `<li class="lg-folder"><span>${escape(folder.dir)}</span><small>${escape(folder.said)}</small>${machine.here || deeds ? `<button type="button" class="lg-word" data-act="folder" data-change="${folder.busy ? "stop" : "forget"}" data-dir="${escape(folder.dir)}">${folder.busy ? "停止" : "移除"}</button>` : ""}</li>${found.filter((login) => login.under === folder.dir).map(lineOf).join("")}`)
          .join("")}${machine.folders.length ? "" : `<li class="lg-folder lg-soft">尚未添加</li>`}</ul></section>`
    : "";
  const others = machines.filter((one) => one !== machine && !one.owner);
  const reach = reachDrawing({ machine: pickable(machine), agents: families.map((family) => ({ agent: family, connected: here.some((login) => login.family === family && login.standing !== "found") })), others: others.map(pickable) } as never, 680, (more) => `另有 ${more} 台`);
  const missing = machine.missing ?? [];
  return `<div class="lg-body lg-sheet">
    <div class="ao-reach" title="${machine.here ? "本机" : "该机器"}已接入的智能体，可经 Fastctx 到达这些机器">${reach}</div>
    <div class="ao-cards">${cards}${folders}</div>
    ${missing.length ? `<p class="lg-note">${machine.here ? "本机" : "该机器"}未找到 ${missing.map(escape).join("、")}</p>` : ""}
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
