// How this computer reaches a hub, drawn rather than said (`world-gui.md`,
// 设置): the computer a wash on its stand, a line of fine ink to the hub's
// clay blot. Whole while connected, dashed while connecting, broken while
// not; the address and since when are said when it is pointed at.

import { Sketch } from "../pen/sketch";

export type HubReach = "online" | "connecting" | "stopped";

export function hubLine(reach: HubReach, seed: string): string {
  const sheet = new Sketch(`hub-line-${seed}`)
    .wash(1, 3, 15, 10, "graphite", 2.4)
    .rule([8.5, 13.4], [8.5, 16], 1.2, "graphite")
    .rule([5, 16.6], [12, 16.6], 1.4, "graphite");
  if (reach === "online") sheet.rule([19, 8.5], [47, 8.5], 1.3, "graphite");
  else if (reach === "connecting") sheet.dashed([19, 8.5], [47, 8.5], 1.2, "graphite", 3.5, 3);
  else sheet.rule([19, 8.5], [29, 8.5], 1.3, "graphite").rule([37, 8.5], [47, 8.5], 1.3, "graphite");
  return sheet.blot([53, 8.5], 3.8, reach === "online" ? "clay" : "graphite").svg(58, 20, `sd-line ${reach}`);
}
