// The panel beside a conversation, without a World: `?page=plan|goal|collab`
// picks the page, `?goal=active|paused|quota|blocked|complete|none` how the
// goal stands, `?shut` the panel closed, `?sample=` the plan, `?queue` a line
// waiting above the input, `?theme=` and `?lang=en` as everywhere.

import "../design/fonts.css";
import "../design/tokens.css";
import "../design/breath.css";
import "../design/base.css";
import "../design/highlight.css";
import "../app/shell.css";
import "../conversation/conversation.css";
import "../app.css";
import "./specimen.css";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { PlanPage } from "../plan/panel";
import { samples } from "../plan/specimen-samples";
import { enPlan, zhPlan } from "../plan/words";
import { CollabPage } from "./collab-page";
import { GoalBar } from "./goal-bar";
import { GoalPage } from "./goal-page";
import { SidePanel, type SidePageName } from "./panel";
import { goalSamples, teamSample } from "./samples";
import type { GoalShown } from "./types";
import { enSide, zhSide } from "./words";

const params = new URLSearchParams(location.search);
document.documentElement.dataset.theme = params.get("theme") ?? "warm-black";
const english = params.get("lang") === "en";
if (english) document.documentElement.lang = "en";
const words = english ? enSide : zhSide;
const planWords = english ? enPlan : zhPlan;
const sample = samples[params.get("sample") ?? "blog"] ?? samples.blog!;
const now = Date.now();
const goals = goalSamples(english, now);
let goal: GoalShown | null = goals[params.get("goal") ?? "active"] ?? null;
const collab = teamSample(english);

const page = document.querySelector<HTMLElement>("#page")!;
page.className = "sps-page";
page.innerHTML = `
  <section class="sps-talk">
    <header class="page-head sps-head"><div class="who"><span class="who-seal">${glyph("claude", 16)}</span><h1 class="who-name">${escape(english ? "Blog comments" : "博客的评论")}</h1></div><div class="page-head-actions"><button class="icon-button sps-toggle" type="button" aria-label="${escape(words.panel)}" title="${escape(words.panel)}">${glyph("panel", 18)}<span class="sps-dot" aria-hidden="true"></span></button><button class="icon-button" type="button" aria-label="find">${glyph("search", 18)}</button><button class="icon-button" type="button" aria-label="more">${glyph("more", 20)}</button></div></header>
    <div class="sps-scroll scroller"><div class="prose sps-said"></div></div>
    <footer class="sps-dock"></footer>
  </section>`;
const said = page.querySelector<HTMLElement>(".sps-said")!;
said.innerHTML = english
  ? `<p class="sps-user">Get the blog's comments working, end to end.</p><p>I've set it as the goal and written a plan on the right. GPT-6.1 Sol is on the back end and GPT-6.1 Luna on the form; I'll write the test once both hand back.</p>`
  : `<p class="sps-user">把博客的评论从头到尾做通。</p><p>已经设成目标，计划写在右边。GPT-6.1 Sol 在做后端，GPT-6.1 Luna 在做表单；两边交回之后我来写那个测试。</p>`;
const dock = page.querySelector<HTMLElement>(".sps-dock")!;

const planPage = new PlanPage(planWords, {
  facts: (id) => sample.facts[id] ?? null,
  save: async () => ({ ok: true }),
  answer: () => {},
});
planPage.show({ source: sample.source, version: sample.version, when: sample.when });

const goalPage = new GoalPage(words.goal, {
  save: async (text) => {
    goal = goal ? { ...goal, objective: text, version: goal.version + 1 } : { objective: text, version: 1, status: "active", worked: 0, since: Date.now() };
    drawGoal();
    return true;
  },
  pause: () => setStatus("paused"),
  resume: () => setStatus("active"),
  clear: () => {
    goal = null;
    drawGoal();
  },
});
const collabPage = new CollabPage(words.collab, words.goal, () => {});
collabPage.show({ agent: "claude", name: "" }, params.get("collab") === "none" ? [] : collab);

// `nogoal`: a Codex conversation opened in Codex's own app, which takes no goal.
const panel = new SidePanel(words, { plan: planPage, ...(params.has("nogoal") ? {} : { goal: goalPage }), collab: collabPage },{ changed: () => drawToggle() }, (params.get("page") as SidePageName | null) ?? "plan");

const bar = new GoalBar(words.goal, {
  pause: () => setStatus("paused"),
  resume: () => setStatus("active"),
  clear: () => {
    goal = null;
    drawGoal();
  },
  expand: () => panel.show("goal"),
});

if (params.has("queue")) {
  const queue = document.createElement("div");
  queue.className = "send-queue";
  queue.innerHTML = `<ul class="queue-rows"><li class="queue-row"><span class="queue-spot"></span><span class="queue-text">${escape(english ? "Use the same font as the posts in the form." : "表单里用和文章一样的字体。")}</span></li></ul>`;
  dock.append(queue);
}
dock.append(bar.element);
dock.insertAdjacentHTML(
  "beforeend",
  `<div class="sps-composer"><span class="sps-placeholder">${escape(english ? "Reply to Claude" : "回复 Claude")}</span><span class="sps-send">${glyph("send", 18)}</span></div>`,
);
page.append(panel.element);

const toggle = page.querySelector<HTMLButtonElement>(".sps-toggle")!;
toggle.addEventListener("click", () => panel.toggle());
const narrow = matchMedia("(max-width: 760px)");
const place = () => panel.over(narrow.matches);
narrow.addEventListener("change", place);
place();

function setStatus(status: GoalShown["status"]): void {
  if (!goal) return;
  goal = { ...goal, status, since: Date.now(), paused: status === "paused" ? { by: "user", until: null } : null };
  drawGoal();
}

function drawGoal(): void {
  bar.show(goal);
  goalPage.show(goal);
}

function drawToggle(): void {
  toggle.setAttribute("aria-pressed", String(panel.open));
  toggle.classList.toggle("news", panel.hasNews);
  page.classList.toggle("open", panel.open);
}

drawGoal();
if (!params.has("shut")) panel.show();
// A page with news while not shown, for looking at its dot.
if (params.has("news")) panel.news(params.get("news") as SidePageName);
drawToggle();
