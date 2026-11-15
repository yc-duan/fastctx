// The windows the agents' pages open over themselves: what connecting a
// login, taking it out, or opening its workbench changes, told before it is
// done and struck off as it is done; a person a login is to assist, with or
// without a limit; the models a login offers; a login's week in dollars; a
// day picked for an assistance to end on.

import { Sheet } from "../app/confirm";
import { glyph } from "../conversation/glyphs";
import { strip } from "./charts";
import { byId, daysOf, deeds, familyName, grants, grantsOf, NOW, people, plateCheck, plateOf, plates, usable, type Login } from "./data";
import { whole } from "./format";
import { plateSaid } from "./login-page";
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
  let limited = true;
  let percent = 25;
  let custom = "";
  let until: { said: string; at: Date } | null = null;
  sheet.body.innerHTML = `${sheet.head("添加协助", "关闭")}
    <p class="as-of">${escape(login.who)}　${escape(familyName(login.family))}　${escape(login.machine)}</p>
    <div class="as-form">
      <span class="as-label">用户</span>
      <div><input class="as-input" name="who" type="text" placeholder="账号名" autocomplete="off" spellcheck="false"><div class="as-names"></div></div>
      <span class="as-label">额度</span>
      <div>
        <div class="as-modes">
          <button type="button" class="as-mode chosen" data-mode="limit"><b>设上限</b><small>对方仅可见本人额度与用量</small></button>
          <button type="button" class="as-mode" data-mode="none"><b>不设上限</b><small>对方可见整个号的额度</small></button>
        </div>
        <div class="as-swap">
          <div class="as-percent"></div>
          <p class="as-shared off">共用全部额度　每周 ${whole(plate)}${fable ? `　Fable 至多 ${whole(plate / 2)}` : ""}</p>
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
  const comes = () => `每周 ${whole((plate * percent) / 100)}${fable ? `　Fable 至多 ${whole((plate * percent) / 200)}` : ""}`;
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
    go.disabled = !typed || taken.has(typed) || (people.length > 0 && !people.includes(typed));
  };
  input.addEventListener("input", listed);
  percents.addEventListener("input", (event) => {
    custom = (event.target as HTMLInputElement).value.replace(/[^0-9]/g, "").slice(0, 3);
    const typed = Number(custom);
    if (typed >= 1 && typed <= 100) percent = typed;
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
    } else if (choice) {
      percent = Number(choice.dataset.value);
      custom = "";
      paint();
    } else if (target.closest("[data-do]") && !go.disabled) {
      const person = input.value.trim().toLowerCase();
      if (deeds) {
        go.disabled = true;
        sheet.busy = true;
        deeds.assist(login, person, limited ? percent : null, until?.at ?? null).then(
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
      grants.push({ login: login.id, person, limit: limited ? percent : null, used: 0, fable: 0, until: until?.said ?? null });
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
    if (deeds) void deeds.assist(login, grant.person, grant.limit, until?.at ?? null).then(() => draw(), (error: unknown) => say(error instanceof Error ? error.message : "更改失败"));
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

/** A login's week in dollars, set by its owner. Under the figure, where it
 *  stands among what the last weeks came to by the readings: the line is
 *  the figure, and can be dragged to where the dots are. */
on("plate", () => {
  const login = current();
  const read = usable(daysOf(login))?.weeks ?? [];
  const sheet = new Sheet("lg-plate");
  const weeks = read.map((week) => ({ value: week.value, said: `${week.start.getMonth() + 1}/${week.start.getDate()} 起一周　按读数推算 ${whole(week.value)}` }));
  let set = plateOf(login);
  sheet.body.innerHTML = `${sheet.head(`周额度　${login.who}`, "关闭")}
    <p class="cs-lead">此登录每周的总额度，是百分比与美元换算的基准。</p>
    <div class="choice-row"><span class="choice-name">每周</span><div><label class="as-custom as-dollars chosen"><span>$</span><input name="plate" type="text" inputmode="numeric" value="${set}" autocomplete="off"></label></div></div>
    ${weeks.length ? `<div class="ps-strip" title="每个点是一周：账本记下的额度，除以官方读数的已用比例。拖动竖线调整设定值。"></div><p class="ps-said"></p>` : `<p class="lg-edit-note">尚无完整一周的读数可供对照。</p>`}
    <footer class="cs-foot"><button type="button" class="sheet-go" data-do>完成</button></footer>`;
  const input = sheet.body.querySelector<HTMLInputElement>("input")!;
  const holder = sheet.body.querySelector<HTMLElement>(".ps-strip");
  let scale: { low: number; high: number; left: number; right: number } | null = null;
  const said = () => {
    const check = plateCheck(read.map((week) => week.value), set);
    const line = sheet.body.querySelector<HTMLElement>(".ps-said");
    if (line && check) line.innerHTML = `读数推算 ${whole(check.read)}${plateSaid(check) || "　"}`;
  };
  const paint = () => {
    if (!holder) return;
    const drawn = strip(login.id, weeks, set, Math.floor(holder.clientWidth) || 500);
    holder.innerHTML = drawn.svg;
    scale = drawn;
    said();
  };
  /** Moves the line to a figure without drawing the dots again. */
  const move = (value: number) => {
    set = Math.max(100, Math.round(value / 50) * 50);
    input.value = String(set);
    const mark = holder?.querySelector<SVGGElement>(".lgc-set");
    if (mark && scale) {
      const at = scale.left + ((Math.min(scale.high, Math.max(scale.low, set)) - scale.low) / (scale.high - scale.low)) * (scale.right - scale.left);
      mark.setAttribute("transform", `translate(${at.toFixed(1)} 0)`);
      mark.querySelector("text")!.textContent = `$${set.toLocaleString("en-US")}`;
    }
    said();
  };
  holder?.addEventListener("pointerdown", (down) => {
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
      set = typed;
      paint();
    }
  });
  sheet.body.querySelector("[data-do]")!.addEventListener("click", () => {
    if (deeds) {
      sheet.busy = true;
      deeds.plate(login, set).then(
        () => {
          sheet.busy = false;
          sheet.close();
          draw();
        },
        (error: unknown) => {
          sheet.busy = false;
          say(error instanceof Error ? error.message : "保存失败");
        },
      );
      return;
    }
    plates[login.id] = set;
    sheet.close();
    draw();
  });
  sheet.show();
  paint();
});
