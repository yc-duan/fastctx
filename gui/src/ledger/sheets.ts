// The windows the agents' pages open over themselves: what connecting a
// login, taking it out, or opening its workbench changes, told before it is
// done and struck off as it is done; a person a login is to assist, with or
// without a limit; the models a login offers; what a login's week is
// expected to hold; its week's usage reset by hand; a day picked for an
// assistance to end on.

import { Sheet } from "../app/confirm";
import { glyph } from "../conversation/glyphs";
import { strip } from "./charts";
import { byId, daysOf, deeds, expectOf, familyName, grants, grantsOf, NOW, people, pinned, plateOf, sample, usable, weeksMiddle, type Login } from "./data";
import { whole } from "./format";
import { rule } from "./marks";
import { draw, goTo, on, say, state } from "./state";
import { choices, dayPicker, escape, menu } from "./ui";

const current = (): Login => byId.get((state.place as { id: string }).id)!;

/** Lights what a deed touched for a moment, wherever it is drawn. */
function lit(key: string): void {
  state.fresh = key;
  draw();
  window.setTimeout(() => {
    if (state.fresh === key) state.fresh = null;
  }, 2400);
}

type Change = "attach" | "detach" | "bench-on" | "bench-off";

function change(login: Login, kind: Change): void {
  // The app tells what a change does as the World previews it, files and all.
  if (deeds) {
    if (kind === "attach") deeds.attach(login);
    else if (kind === "detach") deeds.detach(login);
    else deeds.bench(login, kind === "bench-on");
    return;
  }
  const sheet = new Sheet("lg-change");
  const where = `${login.machine} 上的 ${login.who}`;
  const own = login.family === "claude" || login.family === "codex";
  const told: Record<Change, { title: string; lead: string; cards: [string, string, string][]; go: string; stamp: string }> = {
    attach: {
      title: `接入 ${where}`,
      lead: "接入后，此登录的对话可在你的所有机器上执行命令。",
      cards: [["换上", "Fastctx 的 run", "执行命令、读写文件，并记入活动"], own ? ["关掉", "自带的 Bash", "命令统一经由 run"] : ["保留", "自带的 shell", "两者并用，仅经 run 的命令记入活动"], ["不动", "写文件、待办、子代理、联网搜索", "保持不变"]],
      go: login.standing === "renew" ? "更新接入" : "接入",
      stamp: "已接入",
    },
    detach: { title: `移除 ${where}`, lead: "还原为接入前的状态。", cards: [["移除", "Fastctx 的 run", "不再到达其他机器"], ["还原", "自带的 Bash", "恢复接入前的设置"], ["保留", "已记录的活动与用量", "记录不删除"]], go: "移除", stamp: "已移除" },
    "bench-on": { title: `开启工作台　${login.who}`, lead: "开启后可在 Fastctx 中使用此登录对话。", cards: [["启用", "对话", "可在新对话中选择此登录"], ["启用", "用量与额度记录", "逐请求记录，保存在此机器"], ["变更", "免逐项批准", "仅限 Fastctx 发起的对话"]], go: "开启", stamp: "已开启" },
    "bench-off": { title: `关闭工作台　${login.who}`, lead: "关闭后不再由 Fastctx 发起对话，接入保持不变。", cards: [["停止", "Fastctx 发起的对话", "进行中的一轮会完成"], ["停止", "用量与额度记录", "已有记录保留"], ["结束", "对他人的协助", "受协助者无法再使用"]], go: "关闭", stamp: "已关闭" },
  };
  const tell = told[kind];
  sheet.body.innerHTML = `${sheet.head(tell.title, "关闭")}
    <p class="cs-lead">${escape(tell.lead)}</p>
    <ul class="cs-cards">${tell.cards.map(([kindOf, what, more]) => `<li class="cs-card"><span class="cs-kind">${kindOf}</span><span class="cs-what"><b>${escape(what)}</b><small>${escape(more)}</small><i class="cs-strike">${rule(2)}</i></span></li>`).join("")}</ul>
    <footer class="cs-foot"><span class="cs-stamp">${tell.stamp}</span><button type="button" class="sheet-go${kind === "detach" || kind === "bench-off" ? " danger" : ""}" data-do>${tell.go}</button></footer>`;
  sheet.body.querySelector<HTMLElement>("[data-do]")!.addEventListener("click", (event) => {
    (event.currentTarget as HTMLButtonElement).disabled = true;
    sheet.busy = true;
    const cards = [...sheet.body.querySelectorAll<HTMLElement>(".cs-card")];
    cards.forEach((card, index) => window.setTimeout(() => card.classList.add("done"), 260 + index * 420));
    const after = 260 + cards.length * 420;
    window.setTimeout(() => sheet.body.querySelector(".cs-stamp")!.classList.add("shown"), after + 80);
    window.setTimeout(() => {
      sheet.busy = false;
      sheet.close();
      if (kind === "attach") login.standing = "in";
      else if (kind === "detach") {
        login.standing = "found";
        login.workbench = false;
      } else if (kind === "bench-on") {
        login.workbench = true;
        login.five ??= { used: 4, resets: "4 小时 51 分后" };
        login.week ??= { used: 3, resets: "周日 10:00" };
        if (login.family === "claude") login.fable ??= 1;
      } else login.workbench = false;
      if (state.place.kind === "login" && !login.workbench) goTo({ kind: "attach", machine: login.machine });
      lit(login.id);
    }, after + 950);
  });
  sheet.show();
}

on("attach", (button) => change(byId.get(button.dataset.id!)!, "attach"));
on("detach", (button) => change(byId.get(button.dataset.id!)!, "detach"));
on("bench", (button) => {
  const login = byId.get(button.dataset.id!)!;
  change(login, login.workbench ? "bench-off" : "bench-on");
});

on("line-more", (anchor) => {
  const login = byId.get(anchor.dataset.id!)!;
  menu.open(
    anchor,
    login.who,
    [
      ...(login.workbench ? [{ act: "usage", icon: "rows" as const, label: "用量与调用" }, { act: "talk", icon: "plus" as const, label: "新对话" }, { act: "reread", icon: "scan" as const, label: "重新读取额度" }] : []),
      ...(login.family === "claude" && login.standing === "in" ? [{ act: "models", icon: "tune" as const, label: "模型…" }] : []),
      { act: "copy", icon: "copy" as const, label: "复制目录" },
      { rule: true as const },
      { act: "detach", icon: "trash" as const, label: "移除", danger: true },
    ],
    (act) => {
      if (act === "usage") goTo({ kind: "login", id: login.id, tab: "usage" });
      else if (act === "talk") (deeds ? deeds.newTalk(login) : say(`新对话：${login.who}（样品未实现）`));
      else if (act === "reread") (deeds ? deeds.reread(login) : say("已请求重新读取额度"));
      else if (act === "models") (deeds ? deeds.models(login) : say("模型（样品未实现）"));
      else if (act === "copy") {
        void navigator.clipboard?.writeText(login.dir).catch(() => {});
        say("已复制目录");
      } else change(login, "detach");
    },
  );
});

const dayOf = (at: Date) => `${at.getMonth() + 1} 月 ${at.getDate()} 日`;
const hours = ["09:00", "12:00", "18:00", "24:00"];

/** Until when, in one line that never grows: no end, or a day and an hour,
 *  shown in a field beside the choice. The month to pick the day in opens
 *  over the sheet, under the field, so the sheet keeps its size. */
const untilRow = () =>
  `<div class="as-until">${choices("as-until", [["none", "不限期"], ["day", "截止日期"]], "none")}<button type="button" class="lg-field as-date off" data-act="as-date"></button></div>`;

function wireUntil(sheet: Sheet, changed: (until: { said: string; at: Date } | null) => void): void {
  const row = sheet.body.querySelector<HTMLElement>(".as-until")!;
  const field = row.querySelector<HTMLElement>(".as-date")!;
  let ends = false;
  let day = new Date(NOW.getFullYear(), NOW.getMonth(), NOW.getDate() + 7);
  let hour = "18:00";
  let pop: HTMLElement | null = null;
  const tell = () => {
    field.innerHTML = `${dayOf(day)} ${hour}${glyph("chevron", 12)}`;
    field.classList.toggle("off", !ends);
    const [hours, minutes] = hour.split(":").map(Number);
    changed(ends ? { said: `${dayOf(day)} ${hour}`, at: new Date(day.getFullYear(), day.getMonth(), day.getDate(), hours!, minutes!) } : null);
  };
  const close = () => {
    pop?.remove();
    pop = null;
  };
  const open = () => {
    if (pop) return close();
    const made = document.createElement("div");
    made.className = "lg-pop";
    made.append(dayPicker((picked) => {
      day = picked;
      tell();
    }, day));
    const times = document.createElement("div");
    times.className = "lg-pop-list";
    const paint = () => (times.innerHTML = hours.map((one) => `<button type="button" class="${one === hour ? "chosen" : ""}" data-hour="${one}">${one}</button>`).join(""));
    times.addEventListener("click", (event) => {
      const picked = (event.target as HTMLElement).closest<HTMLElement>("[data-hour]");
      if (!picked) return;
      hour = picked.dataset.hour!;
      paint();
      tell();
    });
    paint();
    made.append(times);
    const box = field.getBoundingClientRect();
    made.style.left = `${Math.round(box.left)}px`;
    made.style.top = `${Math.round(box.bottom + 6)}px`;
    // Inside the sheet's own layer, or the sheet would stand over it.
    sheet.dialog.append(made);
    pop = made;
    // Kept on the screen: over the field where there is no room under it.
    const drawn = made.getBoundingClientRect();
    if (drawn.bottom > innerHeight - 8) made.style.top = `${Math.round(Math.max(8, box.top - 6 - drawn.height))}px`;
  };
  row.addEventListener("click", (event) => {
    const target = event.target as HTMLElement;
    const choice = target.closest<HTMLElement>("[data-act='as-until']");
    if (choice) {
      for (const one of choice.parentElement!.children) one.classList.toggle("chosen", one === choice);
      ends = choice.dataset.value === "day";
      close();
      tell();
    } else if (target.closest("[data-act='as-date']")) open();
  });
  sheet.dialog.addEventListener("pointerdown", (event) => {
    if (pop && !pop.contains(event.target as Node) && !field.contains(event.target as Node)) close();
  });
  sheet.dialog.addEventListener(
    "keydown",
    (event) => {
      if (event.key !== "Escape" || !pop) return;
      event.preventDefault();
      event.stopPropagation();
      close();
    },
    true,
  );
  tell();
}

/** A person for the login to assist. Three things are set, each a row of
 *  the same form: who; whether they are given a limit, which is also what
 *  they will see of the login, and how much; until when. Nothing chosen
 *  makes the sheet taller or shorter: what one choice needs stands in the
 *  place of what the other does. */
on("add-grant", () => {
  const login = current();
  const sheet = new Sheet("lg-assist");
  const taken = new Set(grantsOf(login).map((grant) => grant.person));
  const plate = plateOf(login);
  const fable = login.family === "claude";
  // Only Claude Code's spending is counted per person; a Codex login's
  // assistance is held to its five-hour line alone.
  const limitable = login.family === "claude";
  let limited = limitable;
  let percent = 25;
  let custom = "";
  /** A share typed that is none. */
  let unreadable = false;
  let until: { said: string; at: Date } | null = null;
  sheet.body.innerHTML = `${sheet.head("添加协助", "关闭")}
    <p class="as-of">${escape(login.who)}　${escape(familyName(login.family))}　${escape(login.machine)}</p>
    <div class="as-form">
      <span class="as-label">用户</span>
      <div><input class="as-input" name="who" type="text" placeholder="账号名" autocomplete="off" spellcheck="false"><div class="as-names"></div></div>
      <span class="as-label">额度</span>
      <div>
        <div class="as-modes">
          ${limitable ? `<button type="button" class="as-mode chosen" data-mode="limit"><b>份额</b><small>对方仅可见本人额度</small></button>` : ""}
          <button type="button" class="as-mode${limitable ? "" : " chosen"}" data-mode="none"><b>${limitable ? "共管" : "不设上限"}</b><small>${limitable ? "可见整个号的额度，可调整各人份额" : "Codex 不按人计额度，可用通用限制里的 5 小时额度暂停协助"}</small></button>
        </div>
        <div class="as-swap">
          <div class="as-percent${limitable ? "" : " off"}"></div>
          <p class="as-shared${limitable ? " off" : ""}">共用全部额度　本周 ${whole(plate)}${fable ? `　Fable 至多 ${whole(plate / 2)}` : ""}</p>
        </div>
      </div>
      <span class="as-label">期限</span>
      ${untilRow()}
    </div>
    <p class="as-warn">${glyph("shield", 16)}<span>使用此登录的订阅额度。任务在本机运行，可随时查看与停止。</span></p>
    <footer class="cs-foot"><button type="button" class="sheet-go" data-do disabled>添加</button></footer>`;
  wireUntil(sheet, (value) => (until = value));
  const input = sheet.body.querySelector<HTMLInputElement>(".as-input")!;
  const names = sheet.body.querySelector<HTMLElement>(".as-names")!;
  const percents = sheet.body.querySelector<HTMLElement>(".as-percent")!;
  const go = sheet.body.querySelector<HTMLButtonElement>("[data-do]")!;
  const comes = () => `本周 ${whole((plate * percent) / 100)}${fable ? `　Fable 至多 ${whole((plate * percent) / 200)}` : ""}`;
  const paint = (keep = false) => {
    if (!keep) percents.innerHTML = `<div class="as-limit">${choices("as-limit", [["10", "10%"], ["25", "25%"], ["50", "50%"]], custom ? "" : String(percent))}<label class="as-custom${custom ? " chosen" : ""}"><input type="text" inputmode="numeric" placeholder="自定" value="${escape(custom)}" aria-label="自定比例"><span>%</span></label><span class="as-comes"></span></div>`;
    percents.querySelector(".as-comes")!.textContent = comes();
  };
  const listed = () => {
    const typed = input.value.trim().toLowerCase();
    names.innerHTML = people
      .filter((name) => !taken.has(name) && typed && name.startsWith(typed) && name !== typed)
      .map((name) => `<button type="button" data-name="${name}">${glyph("person", 14)}${name}</button>`)
      .join("");
    go.disabled = !typed || taken.has(typed) || (people.length > 0 && !people.includes(typed)) || (limited && unreadable);
  };
  input.addEventListener("input", listed);
  percents.addEventListener("input", (event) => {
    // A whole share from 1 to 100; `1.5` or `-20` is none, and nothing is
    // added until it is one.
    custom = (event.target as HTMLInputElement).value.trim().replace(/%$/, "").slice(0, 5);
    const typed = /^\d{1,3}$/.test(custom) ? Number(custom) : Number.NaN;
    unreadable = !!custom && !(typed >= 1 && typed <= 100);
    if (!unreadable && custom) percent = typed;
    listed();
    for (const one of percents.querySelectorAll(".choice")) one.classList.toggle("chosen", !custom && one.getAttribute("data-value") === String(percent));
    percents.querySelector(".as-custom")!.classList.toggle("chosen", !!custom);
    paint(true);
  });
  sheet.body.addEventListener("click", (event) => {
    const target = event.target as HTMLElement;
    const name = target.closest<HTMLElement>("[data-name]");
    const mode = target.closest<HTMLElement>("[data-mode]");
    const choice = target.closest<HTMLElement>("[data-act='as-limit']");
    if (name) {
      input.value = name.dataset.name!;
      listed();
    } else if (mode) {
      limited = mode.dataset.mode === "limit";
      for (const one of mode.parentElement!.children) one.classList.toggle("chosen", one === mode);
      percents.classList.toggle("off", !limited);
      sheet.body.querySelector(".as-shared")!.classList.toggle("off", limited);
      listed();
    } else if (choice) {
      percent = Number(choice.dataset.value);
      custom = "";
      unreadable = false;
      listed();
      paint();
  percents.classList.toggle("off", !limited);
    } else if (target.closest("[data-do]") && !go.disabled) {
      const person = input.value.trim().toLowerCase();
      if (deeds) {
        go.disabled = true;
        sheet.busy = true;
        deeds.assist(login, person, limited ? percent : null, until?.at ?? null, limitable && !limited).then(
          () => {
            sheet.busy = false;
            sheet.close();
            lit(person);
          },
          (error: unknown) => {
            sheet.busy = false;
            go.disabled = false;
            say(error instanceof Error ? error.message : "添加失败");
          },
        );
        return;
      }
      grants.push({ login: login.id, person, limit: limited ? percent : null, used: 0, fable: 0, until: until?.said ?? null, ...(limitable && !limited ? { manages: true } : {}) });
      sheet.close();
      lit(person);
    }
  });
  paint();
  sheet.show();
  input.focus();
});

on("grant-until", (button) => {
  const login = current();
  const grant = grantsOf(login).find((one) => one.person === button.dataset.person)!;
  const value = button.dataset.value!;
  const keep = (until: { said: string; at: Date } | null) => {
    if (deeds) void deeds.assist(login, grant.person, grant.limit, until?.at ?? null, grant.manages).then(() => draw(), (error: unknown) => say(error instanceof Error ? error.message : "更改失败"));
    else {
      grant.until = until?.said ?? null;
      draw();
    }
  };
  if (value !== "pick") return keep(null);
  const sheet = new Sheet("lg-assist");
  let until: { said: string; at: Date } | null = null;
  sheet.body.innerHTML = `${sheet.head(`协助期限　${grant.person}`, "关闭")}<div class="as-form"><span class="as-label">期限</span>${untilRow()}</div><footer class="cs-foot"><button type="button" class="sheet-go" data-do>确定</button></footer>`;
  wireUntil(sheet, (picked) => (until = picked));
  sheet.body.querySelector("[data-do]")!.addEventListener("click", () => {
    sheet.close();
    keep(until);
  });
  sheet.show();
});

on("grant-end", (button) => {
  const login = current();
  const person = button.dataset.person!;
  if (deeds) {
    deeds.endAssist(login, person).then(
      () => {
        state.editing = null;
        draw();
      },
      (error: unknown) => say(error instanceof Error ? error.message : "收回失败"),
    );
    return;
  }
  button.closest<HTMLElement>(".lg-item")!.classList.add("leaving");
  window.setTimeout(() => {
    const at = grants.findIndex((grant) => grant.login === login.id && grant.person === button.dataset.person);
    if (at >= 0) grants.splice(at, 1);
    state.editing = null;
    draw();
    say(`已收回对 ${button.dataset.person} 的协助`);
  }, 420);
});

on("models", () => {
  const login = current();
  if (deeds) return deeds.models(login);
  const sheet = new Sheet("lg-models");
  const offered = [...(login.models ?? [])];
  const paint = () => {
    sheet.body.innerHTML = `${sheet.head(`模型　${login.who}`, "关闭")}
      <p class="cs-lead">使用此登录的所有人从此清单中选择模型。</p>
      <ul class="lm-list">${offered.map((name) => `<li><span>${escape(name)}</span><button type="button" class="lg-word" data-drop="${escape(name)}"${offered.length === 1 ? " disabled" : ""}>移除</button></li>`).join("")}</ul>
      <div class="choice-row"><span class="choice-name">添加</span><input class="as-input" name="model" type="text" placeholder="模型 ID，如 claude-opus-5-6" autocomplete="off" spellcheck="false"></div>
      <footer class="cs-foot"><button type="button" class="sheet-go" data-do>完成</button></footer>`;
    sheet.anew();
  };
  sheet.body.addEventListener("click", (event) => {
    const target = event.target as HTMLElement;
    const drop = target.closest<HTMLElement>("[data-drop]");
    if (drop) {
      offered.splice(offered.indexOf(drop.dataset.drop!), 1);
      paint();
    } else if (target.closest("[data-do]")) sheet.close();
  });
  sheet.body.addEventListener("keydown", (event) => {
    const input = event.target as HTMLInputElement;
    if (event.key !== "Enter" || input.name !== "model") return;
    event.preventDefault();
    if (input.value.trim()) offered.push(input.value.trim());
    paint();
  });
  paint();
  sheet.show();
});

/** What a login's owner expects its week to hold, and how far a week's
 *  figure may stand from it. Under them, where that stands among what the
 *  last weeks came to by the readings: the line is what is expected, the
 *  wash about it the reach, and the line can be dragged to where the dots
 *  are. Nothing chosen makes the sheet taller or shorter. */
const SWINGS = ["3", "5", "8"];
on("expect", () => {
  const login = current();
  const read = usable(daysOf(login))?.weeks ?? [];
  const sheet = new Sheet("lg-plate");
  const weeks = read.map((week) => ({ value: week.value, said: `${week.start.getMonth() + 1}/${week.start.getDate()} 起一周　按读数实测 ${whole(week.value)}` }));
  const set = { ...expectOf(login) };
  const middle = weeksMiddle(read.map((week) => week.value));
  sheet.body.innerHTML = `${sheet.head(`周额度　${login.who}`, "关闭")}
    <p class="cs-lead">每周开始时，按上周的读数与账本定出本周周额度，只在预期值的波动幅度内变动。</p>
    <div class="choice-row"><span class="choice-name">预期</span><div><label class="as-custom as-dollars chosen"><span>$</span><input name="base" type="text" inputmode="numeric" value="${Math.round(set.base)}" autocomplete="off"></label></div></div>
    <div class="choice-row"><span class="choice-name">波动幅度</span><div class="ps-swing"></div></div>
    <div class="ps-strip" title="每个点是一周：账本记下的额度，除以官方读数的涨幅。拖动竖线调整预期值。"></div>
    <p class="ps-said"></p>
    <footer class="cs-foot"><button type="button" class="sheet-go" data-do>完成</button></footer>`;
  const input = sheet.body.querySelector<HTMLInputElement>("input")!;
  const holder = sheet.body.querySelector<HTMLElement>(".ps-strip")!;
  const swings = sheet.body.querySelector<HTMLElement>(".ps-swing")!;
  let scale: { low: number; high: number; left: number; right: number } | null = null;
  const said = () => {
    const line = sheet.body.querySelector<HTMLElement>(".ps-said")!;
    if (middle === null) {
      line.textContent = "尚无完整一周的读数可供对照。";
      return;
    }
    const side = pinned(read.map((week) => week.value), set);
    const off = Math.abs(middle - set.base) / set.base >= 0.03;
    line.innerHTML = `近 ${read.length} 周实测居中 ${whole(middle)}${side ? `　<span class="lg-caution">近两周${side === "low" ? "低于下沿" : "高于上沿"}</span>` : ""}${off ? `　<button type="button" class="lg-word lg-accent" data-take>采用 ${whole(Math.round(middle / 10) * 10)}</button>` : ""}`;
  };
  const paint = () => {
    swings.innerHTML = choices("swing", SWINGS.map((value): [string, string] => [value, `±${value}%`]), String(set.swing));
    const drawn = strip(login.id, weeks, set, Math.floor(holder.clientWidth) || 500);
    holder.innerHTML = drawn.svg;
    scale = drawn;
    said();
  };
  /** Moves the line to a figure without drawing the dots again. */
  const move = (value: number) => {
    set.base = Math.max(100, Math.round(value / 10) * 10);
    input.value = String(set.base);
    if (scale) {
      const at = scale.left + ((Math.min(scale.high, Math.max(scale.low, set.base)) - scale.low) / (scale.high - scale.low)) * (scale.right - scale.left);
      const half = ((set.base * set.swing * 2) / 100 / (scale.high - scale.low)) * (scale.right - scale.left) / 2;
      for (const mark of holder.querySelectorAll<SVGGElement>(".lgc-set")) mark.setAttribute("transform", `translate(${at.toFixed(1)} 0)`);
      const reach = holder.querySelector<SVGRectElement>(".lgc-reach");
      reach?.setAttribute("x", (-half).toFixed(1));
      reach?.setAttribute("width", (half * 2).toFixed(1));
      holder.querySelector(".lgc-set-figure")!.textContent = `$${set.base.toLocaleString("en-US")}`;
    }
    said();
  };
  holder.addEventListener("pointerdown", (down) => {
    if (!scale) return;
    const box = holder.getBoundingClientRect();
    const at = (event: PointerEvent) => move(scale!.low + ((event.clientX - box.left - scale!.left) / (scale!.right - scale!.left)) * (scale!.high - scale!.low));
    holder.setPointerCapture(down.pointerId);
    holder.classList.add("dragging");
    at(down);
    const end = () => {
      holder.classList.remove("dragging");
      holder.removeEventListener("pointermove", at);
      paint();
    };
    holder.addEventListener("pointermove", at);
    holder.addEventListener("pointerup", end, { once: true });
    holder.addEventListener("pointercancel", end, { once: true });
  });
  input.addEventListener("input", () => {
    const typed = Number(input.value.replace(/[^0-9]/g, ""));
    if (typed >= 100) {
      set.base = typed;
      paint();
    }
  });
  sheet.body.addEventListener("click", (event) => {
    const target = event.target as HTMLElement;
    const swing = target.closest<HTMLElement>("[data-act='swing']");
    if (swing) {
      set.swing = Number(swing.dataset.value);
      paint();
    } else if (target.closest("[data-take]") && middle !== null) {
      set.base = Math.round(middle / 10) * 10;
      input.value = String(set.base);
      paint();
    } else if (target.closest("[data-do]")) {
      const done = () => {
        sheet.busy = false;
        sheet.close();
        draw();
      };
      if (deeds) {
        sheet.busy = true;
        deeds.expect(login, set).then(done, (error: unknown) => {
          sheet.busy = false;
          say(error instanceof Error ? error.message : "保存失败");
        });
      } else {
        sample?.expect(login, set);
        done();
      }
    }
  });
  sheet.show();
  paint();
});

/** The week's usage reset by its owner, after the vendor reset the quota or
 *  a part of it: to nothing, or to a part of the week, everyone's use so
 *  far scaled alike. What it will make of each is shown before it is done.
 *  Nothing chosen makes the sheet taller or shorter. */
on("reset-usage", () => {
  const login = current();
  const days = usable(daysOf(login));
  const sheet = new Sheet("lg-reset");
  const plate = plateOf(login);
  const people = (days?.people ?? []).filter((one) => one.week > 0 || one.fable > 0);
  const total = (people.reduce((sum, one) => sum + one.week, 0) / plate) * 100;
  const fable = people.reduce((sum, one) => sum + one.fable, 0);
  // The vendor's reading, where it stands under what the ledger comes to,
  // is what a reset most likely means to reach.
  const read = login.week && !login.week.reset && login.week.used < total - 1 ? login.week.used : null;
  let all = read === null;
  let typed = read === null ? "" : String(read);
  sheet.body.innerHTML = `${sheet.head(`重置用量　${login.who}`, "关闭")}
    <p class="cs-lead">厂商重置或部分重置额度后使用：所有人本周已用按同一比例折算。可在重置记录中撤回。</p>
    <div class="choice-row"><span class="choice-name">重置为</span><div class="rs-to"></div></div>
    <div class="rs-list"></div>
    <p class="rs-fable"></p>
    <footer class="cs-foot"><button type="button" class="sheet-go" data-do>重置</button></footer>`;
  const to = sheet.body.querySelector<HTMLElement>(".rs-to")!;
  const list = sheet.body.querySelector<HTMLElement>(".rs-list")!;
  const go = sheet.body.querySelector<HTMLButtonElement>("[data-do]")!;
  /** The part of the week asked for, or none where what is typed is none
   *  or would raise the usage. */
  const target = (): number | null => {
    if (all) return 0;
    const value = /^\d{1,3}$/.test(typed.trim()) ? Number(typed) : Number.NaN;
    return value >= 0 && value < total ? value : null;
  };
  const paint = (keep = false) => {
    if (!keep)
      to.innerHTML = `<div class="as-limit">${choices("reset-to", [["all", "全部清零"], ["part", "指定比例"]], all ? "all" : "part")}<label class="as-custom${all ? "" : " chosen"}"><input type="text" inputmode="numeric" placeholder="比例" value="${escape(typed)}" aria-label="重置到的比例"><span>%</span></label><span class="as-comes">${read === null ? "" : `官方 7 天读数 ${read}%`}</span></div>`;
    const at = target();
    const scale = at === null ? 1 : total > 0 ? at / total : 0;
    const row = (name: string, before: string, after: string, more = "") => `<div class="rs-row${more}"><span>${name}</span><span>${before}</span><span class="lg-soft">→</span><span>${after}</span></div>`;
    list.innerHTML =
      people.map((one) => row(one.name === null ? (login.lent ? escape(login.lent.by) : "我") : escape(one.name), whole(one.week), at === null ? "" : whole(one.week * scale))).join("") +
      row("合计", `${Math.round(total)}%`, at === null ? "" : `${Math.round(at)}%`, " rs-sum");
    sheet.body.querySelector<HTMLElement>(".rs-fable")!.innerHTML =
      at === null
        ? `<span class="lg-caution">须低于现有合计 ${Math.round(total)}%</span>`
        : login.family === "claude"
          ? `Fable ${whole(fable)} → ${whole(fable * scale)}${login.fableHeld && (fable * scale) / plate < 0.5 ? "，恢复放行" : ""}。厂商是否同样返还 Fable，读数无法得知。`
          : "";
    go.disabled = at === null;
  };
  to.addEventListener("input", (event) => {
    typed = (event.target as HTMLInputElement).value.replace(/%$/, "").slice(0, 4);
    all = false;
    to.querySelector(".choice.chosen")?.classList.remove("chosen");
    to.querySelector("[data-value='part']")?.classList.add("chosen");
    to.querySelector(".as-custom")?.classList.add("chosen");
    paint(true);
  });
  sheet.body.addEventListener("click", (event) => {
    const pressed = event.target as HTMLElement;
    const choice = pressed.closest<HTMLElement>("[data-act='reset-to']");
    if (choice) {
      all = choice.dataset.value === "all";
      paint();
      if (!all) to.querySelector<HTMLInputElement>("input")?.focus();
    } else if (pressed.closest("[data-do]") && !go.disabled) {
      const at = target()!;
      const done = () => {
        sheet.busy = false;
        sheet.close();
        draw();
      };
      if (deeds) {
        sheet.busy = true;
        deeds.resetUsage(login, at).then(done, (error: unknown) => {
          sheet.busy = false;
          say(error instanceof Error ? error.message : "重置失败");
        });
      } else {
        sample?.resetUsage(login, at);
        done();
      }
    }
  });
  sheet.show();
  paint();
});

on("undo-reset", () => {
  const login = current();
  if (deeds) void deeds.undoReset(login).then(() => draw(), (error: unknown) => say(error instanceof Error ? error.message : "撤回失败"));
  else {
    sample?.undoReset(login);
    draw();
  }
});
