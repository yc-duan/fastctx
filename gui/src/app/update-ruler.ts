// The versions of the network as a rule drawn against a straight edge, at
// the top of the machines page's update view (`world-gui.md`, 界面的结构): a
// tick a release, the newest at the right named in clay; over each tick a
// bar swept with a marker as tall as how many machines run that release, the
// part of it online in ink; a machine on its way a clay dot travelling the
// rule toward the release it goes to; the network's minimum a pencilled line,
// the hub's release marked under its tick. The page's one bold thing.

import { escape } from "../conversation/prose";
import { dashed, dot, ruled, seeded, swatch } from "../pen/shapes";
import { compareVersions, type MachineUpdate } from "../world/updates";
import { runningOf, shortVersion } from "./update-marks";
import type { UpdateViewWords } from "./update-view-words";

export interface RulerNow {
  machines: MachineUpdate[];
  /** The newest releases the network knows of. */
  newest: string[];
  hub: string | null;
  minimum: string | null;
  /** The release line every version shown shares. */
  line: string | null;
  width: number;
  phone: boolean;
}

/** How many releases the rule shows; older ones are one tick together. */
const TICKS = { desk: 8, phone: 6 };

const path = (d: string, kind: string) => `<path class="${kind}" d="${d}"/>`;

export function rulerOf(now: RulerNow, words: UpdateViewWords): string {
  const { phone, width, line } = now;
  const height = phone ? 132 : 200;
  const baseline = height - (phone ? 44 : 52);
  const tall = baseline - (phone ? 34 : 44);
  const running = now.machines.map(runningOf).filter((version): version is string => !!version);
  let versions = [...new Set([...running, ...now.newest, ...(now.hub ? [now.hub] : [])])].sort(compareVersions);
  if (!versions.length || width < 160) return "";
  const most = phone ? TICKS.phone : TICKS.desk;
  const older = versions.length > most ? versions.slice(0, versions.length - most + 1) : [];
  if (older.length) versions = versions.slice(older.length);
  const ticks: { label: string; versions: string[] }[] = [
    ...(older.length ? [{ label: words.older, versions: older }] : []),
    ...versions.map((version) => ({ label: version, versions: [version] })),
  ];
  const x0 = phone ? 24 : 56;
  const x1 = width - x0;
  const xOf = (index: number) => (ticks.length === 1 ? (x0 + x1) / 2 : x0 + ((x1 - x0) * index) / (ticks.length - 1));
  const tickOf = (version: string) => ticks.findIndex((tick) => tick.versions.includes(version));
  const svg: string[] = [path(ruled([x0 - 24, baseline], [x1 + 24, baseline], 1.3, seeded("uv-rule")), "uv-rule")];
  const notes: string[] = [];
  const counts = ticks.map((tick) => now.machines.filter((machine) => tick.versions.includes(runningOf(machine) ?? "")));
  const biggest = Math.max(1, ...counts.map((machines) => machines.length));
  const newest = versions[versions.length - 1];
  const barWidth = phone ? 10 : 16;
  ticks.forEach((tick, index) => {
    const x = xOf(index);
    svg.push(path(ruled([x, baseline - 4], [x, baseline + 6], 1.2, seeded(`uv-t-${tick.label}`)), "uv-rule"));
    const label = tick.versions.length > 1 ? tick.label : shortVersion(tick.label, index === 0 && !phone ? null : line);
    notes.push(`<span class="uv-tick${tick.label === newest ? " newest" : ""}" style="left:${x}px;top:${baseline + 12}px">${escape(label)}</span>`);
    const here = counts[index]!;
    if (!here.length) return;
    const awake = here.filter((machine) => machine.online).length;
    const barHeight = Math.max(6, (here.length / biggest) * tall);
    svg.push(path(swatch(x - barWidth / 2, baseline - 6 - barHeight, barWidth, barHeight, seeded(`uv-b-${tick.label}`), 3), "uv-away"));
    notes.push(`<span class="uv-hit" style="left:${x - 16}px;top:${baseline - 26 - barHeight}px;height:${barHeight + 26}px" title="${escape(words.machinesOn(here.length, tick.label))}"></span>`);
    if (awake) svg.push(path(swatch(x - barWidth / 2, baseline - 6 - (awake / biggest) * tall, barWidth, (awake / biggest) * tall, seeded(`uv-a-${tick.label}`), 3), "uv-awake"));
    notes.push(`<span class="uv-count" style="left:${x}px;top:${baseline - 26 - barHeight}px">${here.length}</span>`);
  });
  // Those on their way: a dot along the rule from what they run toward the
  // release they go to, as far as their package has come.
  const moving = now.machines.filter((machine) => machine.stage.kind === "fetching");
  for (const machine of moving) {
    if (machine.stage.kind !== "fetching") continue;
    const from = tickOf(runningOf(machine) ?? "");
    const to = tickOf(machine.stage.version);
    if (from < 0 || to < 0 || to === from) continue;
    const share = machine.stage.total ? Math.min(1, machine.stage.received / machine.stage.total) : 0.05;
    const start = xOf(from);
    const at = start + (xOf(to) - start) * share;
    const y = baseline - 12;
    const random = seeded(`uv-m-${machine.machine}`);
    if (Math.abs(at - start) > 18) svg.push(path(ruled([start + 12, y], [at - 6, y], 1.2, random), "uv-trail"));
    svg.push(path(dot([at, y], phone ? 3 : 4.2, random), "uv-moving"));
    if (!phone && moving.length <= 3) notes.push(`<span class="uv-moving-name" style="left:${at}px;top:${y - 30}px">${escape(machine.machine)}</span>`);
  }
  // The network's minimum, pencilled across its tick.
  const minimum = now.minimum ? tickOf(now.minimum) : -1;
  if (minimum >= 0) {
    svg.push(path(dashed([xOf(minimum), 8], [xOf(minimum), baseline - 4], 1, seeded("uv-min"), 5, 5), "uv-pencil"));
    if (!phone) notes.push(`<span class="uv-note" style="left:${xOf(minimum)}px;top:${baseline + 34}px">${escape(words.minimumMark)}</span>`);
  }
  const hub = now.hub ? tickOf(now.hub) : -1;
  if (hub >= 0 && !phone) {
    const below = minimum === hub ? 52 : 34;
    notes.push(`<span class="uv-note" style="left:${xOf(hub)}px;top:${baseline + below}px">${escape(words.hubMark)}</span>`);
  }
  return `<div class="uv-ruler" style="height:${height}px"><svg width="${width}" height="${height}" viewBox="0 0 ${width} ${height}" aria-hidden="true">${svg.join("")}</svg>${notes.join("")}</div>`;
}
