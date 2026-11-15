// The agents' pages as a specimen: the whole section beside the app, drawn
// from a made-up network (`data.ts`) and pressed through as the real one
// will be, for looking at before it is wired in. Where it stands is in the
// address: `#/quota`, `#/attach/<machine>`, `#/login/<id>/<view>`; `?theme=`
// and `?size=` set the theme and the body's size.

import "../design/fonts.css";
import "../design/tokens.css";
import "../design/base.css";
import "../design/breath.css";
import "../app/sheet.css";
import "../app/talk-menu.css";
import "../app/when.css";
import "../app/agents-overview.css";
import "./ledger.css";

import { installPointerHints } from "../app/pointer-hint";
import { followSwitches, grabSwitches } from "../app/segment-slide";
import { fitInks } from "../app/usage/draw";
import { glyph } from "../conversation/glyphs";
import { installPens } from "../pen/filters";
import { installRules } from "../pen/rules";
import { homePage } from "./home";
import { fillCharts, loginPage } from "./login-page";
import "./sheets";
import { setHost, setNet } from "./data";
import { fakeFeed, fakeNet } from "./fake";
import { actOf, draw, drawnBy, placeOf, state, type How, type Place } from "./state";

const params = new URLSearchParams(location.search);
document.documentElement.style.setProperty("--body-size", `${params.get("size") ?? "16"}px`);
// `?motion=off` holds everything still, as the app does when asked to move
// less: a screenshot then shows a window opened, not the start of its coming.
if (params.get("motion") === "off") document.documentElement.setAttribute("data-reduced", "");

function side(): string {
  const item = (icon: Parameters<typeof glyph>[0], name: string, more = "") =>
    `<a${name === "智能体" ? ` href="#/quota" aria-current="page"` : ""}>${glyph(icon, 18)}<span>${name}</span>${more}</a>`;
  return `<nav class="lg-side">
    <div class="lg-side-net">${glyph("tree", 18)}我的网络</div>
    ${item("agents", "智能体")}${item("history", "活动")}${item("machine", "机器")}
    <h2>工作台</h2>
    ${item("plus", "新对话")}${item("tree", "世界", "<b>1</b>")}${item("folder", "项目")}${item("settings", "对话设置")}
    <h2>项目</h2>
    ${item("folder", "mark50 研发")}${item("claude", "PID 环改定点")}${item("claude", "真机验证")}${item("folder", "ingest 服务")}${item("codex", "批量提交")}
    <h2>对话</h2>
    ${item("codex", "图标导出")}${item("claude", "日志轮转")}
    <div class="lg-side-foot">${item("person", "me", "<b>2</b>")}${item("settings", "设置")}</div>
  </nav>`;
}

setNet(fakeNet());
setHost({ feed: fakeFeed });
installPens();
installRules();
installPointerHints();
followSwitches();

const root = document.getElementById("app")!;
root.innerHTML = `<div class="lg">${side()}<main class="lg-main"></main></div>`;
const main = root.querySelector<HTMLElement>(".lg-main")!;
grabSwitches(root);

drawnBy((how) => {
  const kept = main.querySelector<HTMLElement>(".lg-sheet")?.scrollTop ?? 0;
  main.dataset.arriving = how;
  main.innerHTML = state.place.kind === "login" ? loginPage() : homePage();
  if (how === "quiet") main.querySelector<HTMLElement>(".lg-sheet")?.scrollTo({ top: kept });
  fitInks(main);
  fillCharts(main, how !== "quiet");
});

/** Where a view stands among the views under its head. */
const order = (place: Place) => (place.kind === "quota" ? 0 : place.kind === "attach" ? 1 : ["usage", "calls", "assist", "settings"].indexOf(place.tab));

/** How the page comes, going from one place to another. */
function way(from: Place, to: Place): How {
  if (to.kind === "login") {
    if (from.kind !== "login" || from.id !== to.id) return "forward";
  } else if (from.kind === "login") return "back";
  const step = order(to) - order(from);
  return step > 0 ? "right" : step < 0 ? "left" : "quiet";
}

function arrive(): void {
  const from = state.place;
  const to = placeOf(location.hash);
  if (to.kind === "attach") state.machine = to.machine;
  if (to.kind === "login" && (from.kind !== "login" || from.id !== to.id)) Object.assign(state, { page: 1, who: null, model: null, source: null, outcome: null, split: "none", chart: "days" });
  state.opened = null;
  state.editing = null;
  state.place = to;
  draw(way(from, to));
}
window.addEventListener("hashchange", arrive);

root.addEventListener("click", (event) => {
  const target = event.target as HTMLElement;
  const act = target.closest<HTMLElement>("[data-act]");
  const go = target.closest<HTMLElement>("[data-go]");
  // The nearer of the two is the one pressed.
  if (act && (!go || go.contains(act))) {
    if (!(act as HTMLButtonElement).disabled) actOf(act.dataset.act!)?.(act, event);
    return;
  }
  if (go) location.hash = go.dataset.go!;
});
root.addEventListener("keydown", (event) => {
  const target = event.target as HTMLElement;
  if ((event.key === "Enter" || event.key === " ") && target.matches("[role='link'], [role='button']")) {
    event.preventDefault();
    target.click();
  }
});

let resized = 0;
window.addEventListener("resize", () => {
  window.clearTimeout(resized);
  resized = window.setTimeout(() => draw(), 120);
});

state.place = placeOf(location.hash);
if (state.place.kind === "attach") state.machine = state.place.machine;
draw("page");

// `?press=` names marks to press once the page is drawn, one after another
// (`a|b`), so a screenshot can be taken of what a press opens.
params
  .get("press")
  ?.split("|")
  .forEach((selector, index) => window.setTimeout(() => document.querySelector<HTMLElement>(selector)?.click(), 500 + index * 700));
// `?hover=` names a mark the pointer is then taken onto, for a screenshot of
// what it says.
const hovered = params.get("hover");
if (hovered) window.setTimeout(() => document.querySelector(hovered)?.dispatchEvent(new PointerEvent("pointermove", { bubbles: true })), 2600);
