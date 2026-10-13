import "../design/fonts.css";
import "../design/tokens.css";
import "../design/base.css";
import "./specimen.css";
import { applyTheme } from "../theme";
import { blot, circle, outline, quadratic, seeded } from "../pen/shapes";
import type { MapHub, MapNode } from "../app/map-nodes";
import { MapCanvas, PenLayer, StableNetworkLayout, type Bounds, type CanvasEvent, type CanvasFrame, type CanvasHandle, type CanvasItem, type NetworkLayout, type PenStroke, type Point } from "./index";

const params = new URLSearchParams(location.search);
const select = document.querySelector<HTMLSelectElement>("#world")!;
select.value = params.get("world") ?? "60";
const summary = document.querySelector<HTMLElement>("#summary")!;
const result = document.querySelector<HTMLOutputElement>("#result")!;
const action = document.createElement("div");
action.className = "canvas-action";
action.hidden = true;
const draftSvg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
draftSvg.classList.add("canvas-draft");
draftSvg.setAttribute("aria-hidden", "true");
const draftPath = document.createElementNS("http://www.w3.org/2000/svg", "path");
draftPath.setAttribute("fill", "var(--accent)");
draftSvg.append(draftPath);
let world = makeWorld(select.value);
let layoutEngine = new StableNetworkLayout();
let layout: NetworkLayout;
let level = -1;
let selectedAt: { key: string; point: Point } | null = null;
let changed = false;
let composeCount = 0;
const events: CanvasEvent[] = [];
const surface = new MapCanvas({
  label: "Network drawing. Drag to pan; control-wheel to zoom. Tab reaches every ring and handle.",
  controls: { minus: "Zoom out", plus: "Zoom in", fit: "Fit world", scale: "Scale" },
  minScale: 1 / 64, maxScale: 8, levels: [0.55, 1.2],
  validTarget: (handle, item) => {
    const source = world.nodes.find(n => n.key === handle.source);
    if (!source || item.key === source.key) return false;
    const hub = handle.key.split("|")[1];
    if (item.kind === "hub") return item.key === `hub:${hub}` && world.hubs.some(h => h.id === hub && h.online);
    const target = world.nodes.find(n => n.key === item.key);
    return !!target && target.online === true && target.hold === "mine" && target.hubs.includes(hub!) && Number(target.name.split("-")[1]) % 5 === 0;
  },
  event: onEvent,
  frame: onFrame,
  draftLabel: draft => `${draft.handle.label}. ${draft.valid.length} targets. ${draft.target ?? "No target"}. Tab chooses; Enter drops; Escape cancels.`,
});
const pens = new PenLayer(surface.ink, params.has("live-filter") ? "live-filter" : "texture");
let inkReady: Promise<void> = Promise.resolve();
surface.overlay.append(draftSvg, action);
document.querySelector("#map")!.append(surface.element);
applyTheme(() => { if (layout) compose(); });
update();
select.addEventListener("change", () => { world = makeWorld(select.value); layoutEngine = new StableNetworkLayout(); changed = false; level = -1; update(); surface.fit(); });
document.querySelector("#change")!.addEventListener("click", () => {
  if (!changed) world.nodes.push({ key: "machine:added", name: "new-machine", sub: "", hubs: world.hubs.slice(0, 2).map(h => h.id),
    here: false, online: true, hold: "mine", reaching: false, busy: 0, title: "New machine" });
  else world.nodes = world.nodes.filter(n => n.key !== "machine:added");
  changed = !changed;
  update();
});
document.querySelector<HTMLButtonElement>("#measure")!.addEventListener("click", () => {
  void measure().then(m => { result.textContent = `Pan p95 ${m.pan.p95.toFixed(1)} ms · zoom p95 ${m.zoom.p95.toFixed(1)} ms`; });
});

function makeWorld(mode: string): { hubs: MapHub[]; nodes: MapNode[] } {
  const count = mode === "empty" ? 0 : mode === "small" ? 8 : 60;
  const hubCount = mode === "empty" ? 0 : mode === "crowded" ? 1 : mode === "small" ? 2 : 3;
  const hubs = Array.from({ length: hubCount }, (_, i) => ({ id: `h${i}`, name: `Hub ${i + 1}`, online: true }));
  const nodes = Array.from({ length: count }, (_, i): MapNode => ({
    key: `machine:${String(i + 1).padStart(2, "0")}`, name: `machine-${String(i + 1).padStart(2, "0")}`, sub: i % 4 === 0 ? "Linux" : "",
    hubs: i % 10 === 0 && hubCount > 1 ? hubs.map(h => h.id) : [hubs[i % hubCount].id],
    here: i === 0, online: i % 13 !== 12, hold: "mine", reaching: false,
    busy: i % 3 === 0 ? 1 : 0, title: `Machine ${i + 1}`,
  }));
  return { hubs, nodes };
}
function update(): void {
  layout = layoutEngine.update(world.hubs, world.nodes);
  compose();
  scene();
}
function handleFor(node: MapNode, hub: string): CanvasHandle {
  const p = layout.machines.get(node.key)!, k = layout.hubs.get(hub)!;
  const length = Math.hypot(k[0] - p[0], k[1] - p[1]);
  return { key: `${node.key}|${hub}`, source: node.key, label: `Route ${node.name} to ${hub}`, point: [p[0] + (k[0] - p[0]) / length * 42, p[1] + (k[1] - p[1]) / length * 42] };
}
function scene(): void {
  const items = layout.items.map(item => ({ ...item, sub: level >= 1 ? item.sub ?? "" : "", labelRadius: item.kind === "machine" ? level >= 1 ? 44 : 30 : 14 }));
  const handles = world.nodes.filter(n => n.online && (n.key === surface.selection || params.has("handles")))
    .flatMap(n => n.hubs.filter(h => layout.hubs.has(h)).map(h => handleFor(n, h)));
  surface.setScene(items, handles, layout.bounds);
}
function strokeBounds(points: readonly Point[], padding: number): Bounds {
  const x = Math.min(...points.map(p => p[0])) - padding, y = Math.min(...points.map(p => p[1])) - padding;
  return { x, y, width: Math.max(...points.map(p => p[0])) - x + padding, height: Math.max(...points.map(p => p[1])) - y + padding };
}
function compose(): void {
  composeCount++;
  const strokes: PenStroke[] = [];
  const add = (d: string, bounds: Bounds, pen: PenStroke["pen"], colour: string, opacity = 1) => strokes.push({ d, bounds, pen, colour, opacity });
  // Style is deliberately confined to this specimen; the canvas has no level-specific art.
  const css = getComputedStyle(document.documentElement);
  const ink = css.getPropertyValue("--text").trim(), accent = css.getPropertyValue("--accent").trim();
  for (const item of layout.items) {
    const random = seeded(item.key);
    const offline = item.kind === "machine" && !world.nodes.find(n => n.key === item.key)!.online;
    add(circle(item.point, item.radius, 2, random), strokeBounds([item.point], item.radius + 4), offline ? "dry" : "marker", ink, offline ? 0.5 : 0.9);
    if (item.kind === "hub") add(blot(item.point, 3, random), strokeBounds([item.point], 5), "marker", ink);
    if (item.kind !== "machine") continue;
    const node = world.nodes.find(n => n.key === item.key)!;
    for (const hub of node.hubs) {
      const k = layout.hubs.get(hub)!;
      const dx = k[0] - item.point[0], dy = k[1] - item.point[1], length = Math.hypot(dx, dy);
      const start: Point = [item.point[0] + dx / length * 31, item.point[1] + dy / length * 31];
      const end: Point = [k[0] - dx / length * 15, k[1] - dy / length * 15];
      const bow = (random() - 0.5) * length * 0.18;
      const control: Point = [(start[0] + end[0]) / 2 - dy / length * bow, (start[1] + end[1]) / 2 + dx / length * bow];
      const points = quadratic(start, control, end);
      add(outline(points, 1.5, random), strokeBounds(points, 3), offline ? "dry" : "marker", ink, 0.4);
    }
    if (level >= 1) for (let index = 0; index < node.busy; index++) {
      const p: Point = [item.point[0] + index * 12 - 6, item.point[1] - 39];
      add(blot(p, 3.4, random), strokeBounds([p], 5), "marker", accent);
    }
    if (level >= 2) {
      const arc = Array.from({ length: 20 }, (_, i): Point => [item.point[0] + Math.cos(i / 19 * Math.PI * 0.8) * 35, item.point[1] + Math.sin(i / 19 * Math.PI * 0.8) * 35]);
      add(outline(arc, 2, random), strokeBounds(arc, 3), "marker", accent);
    }
  }
  inkReady = pens.update(strokes);
  void inkReady.catch(error => { result.textContent = String(error); throw error; });
  surface.ink.style.color = ink;
}
function float(key: string, at: Point, drop = false): void {
  selectedAt = { key, point: at };
  action.replaceChildren();
  const label = document.createElement("span");
  label.textContent = drop ? `Drop → ${key}` : key;
  action.append(label);
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = drop ? "Dismiss" : "Action";
  button.addEventListener("click", () => { result.textContent = drop ? "Intent only; no network changed." : `Action · ${key}`; action.hidden = true; surface.reserveLabels([]); });
  action.append(button);
  action.hidden = false;
  positionAction(at);
}
function positionAction(at: Point): void {
  const v = surface.camera.view;
  action.style.left = `${Math.max(8, Math.min(v.width - action.offsetWidth - 8, at[0] + 32))}px`;
  action.style.top = `${Math.max(8, Math.min(v.height - action.offsetHeight - 64, at[1] + 42))}px`;
  surface.reserveLabels([{ x: parseFloat(action.style.left), y: parseFloat(action.style.top), width: action.offsetWidth, height: action.offsetHeight }]);
}
function onEvent(event: CanvasEvent): void {
  // The specimen's evidence log records every event, without truncation.
  events.push(event);
  if (event.kind === "select") {
    if (event.key) float(event.key, event.at);
    else { action.hidden = true; selectedAt = null; surface.reserveLabels([]); }
    scene();
  } else if (event.kind === "context") float(event.key ?? "Empty space", event.at);
  else if (event.kind === "draft") { action.hidden = true; surface.reserveLabels([]); drawDraft(); }
  else if (event.kind === "drop") { draftPath.setAttribute("d", ""); float(event.target, event.at, true); result.textContent = `Drop ${event.handle.source} → ${event.target}`; }
  else if (event.kind === "cancel") { draftPath.setAttribute("d", ""); result.textContent = `Cancel · ${event.reason}`; }
}
function drawDraft(): void {
  const draft = surface.activeDraft;
  if (!draft) return;
  const from = surface.handlePoint(draft.handle.key)!, to = surface.camera.toScreen(draft.point);
  const control: Point = [(from[0] + to[0]) / 2, (from[1] + to[1]) / 2 - Math.hypot(to[0] - from[0], to[1] - from[1]) * 0.1];
  draftPath.setAttribute("d", outline(quadratic(from, control, to), 2.3, seeded(draft.handle.key)));
}
function onFrame(frame: CanvasFrame): void {
  if (level !== frame.view.level) { level = frame.view.level; compose(); scene(); }
  pens.view(frame);
  const step = 32 * frame.view.scale;
  surface.element.style.backgroundImage = "linear-gradient(var(--grid) 1px, transparent 1px),linear-gradient(90deg,var(--grid) 1px,transparent 1px)";
  surface.element.style.backgroundSize = `${step}px ${step}px`;
  surface.element.style.backgroundPosition = `${frame.view.x}px ${frame.view.y}px`;
  if (selectedAt && !action.hidden) {
    const item = layout.items.find(i => i.key === selectedAt!.key);
    if (item) positionAction(surface.camera.toScreen(item.point));
  }
  if (surface.activeDraft) drawDraft();
  summary.textContent = `${world.hubs.length} hubs · ${world.nodes.length} machines · level ${level} · ${frame.hiddenLabels.length} names hidden`;
}
function stats(values: number[]) {
  const sorted = [...values].sort((a, b) => a - b);
  return { samples: values.length, p50: sorted[Math.floor(values.length * 0.5)], p95: sorted[Math.floor(values.length * 0.95)], max: sorted.at(-1)!, over25: values.filter(v => v > 25).length };
}
async function measure() {
  const before = surface.camera.view;
  await inkReady;
  action.hidden = true;
  surface.reserveLabels([]);
  const sample = async (zoom: boolean) => {
    const times: number[] = [];
    let previous = 0;
    // 240 refresh intervals cover four seconds on a 60 Hz screen.
    for (let i = 0; i <= 240; i++) {
      const time = await new Promise<number>(r => requestAnimationFrame(r));
      if (previous) times.push(time - previous);
      previous = time;
      if (zoom) surface.zoom(i < 120 ? 1.006 : 1 / 1.006, [before.width * 0.4, before.height * 0.4]);
      else surface.pan(i < 120 ? 2 : -2, i < 120 ? 0.5 : -0.5);
    }
    return stats(times);
  };
  const pan = await sample(false), zoom = await sample(true);
  surface.camera.set(before);
  surface.refresh();
  return { pan, zoom, composeCount, filters: surface.ink.querySelectorAll("g[filter]").length, tiles: pens.tileCount, visibleTiles: pens.visibleTileCount };
}

// Dev-only entry for real browser probes; not imported by the application.
Object.assign(window, { mapSpecimen: {
  surface, pens, events, measure,
  get ready() { return inkReady; },
  get world() { return world; }, get layout() { return layout; }, get composeCount() { return composeCount; },
  changeTarget(key: string, online: boolean) { const node = world.nodes.find(n => n.key === key); if (node) node.online = online; },
  item(key: string): CanvasItem { return layout.items.find(i => i.key === key)!; },
  update,
} });
