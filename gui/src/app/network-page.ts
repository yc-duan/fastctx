// A network this device is in (`world-gui.md`, 界面的结构): what its server
// is, drawn rather than told; the server's addresses; whether this device's
// accounts there are its administrator or members; how many machines it
// has; the administrator's way to manage the server; and leaving it.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { loginsOf, type Login } from "../world/accounts";
import type { AccountChange } from "../world/accounts";
import { circle, ruled, seeded, type Point } from "../pen/shapes";
import type { Confirm } from "./confirm";
import { networksOf } from "./network";
import { link } from "./places";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";

export interface NetworkWords {
  title: string;
  /** Under the drawing: what a server does, in one line. */
  serverIs: string;
  address: string;
  you: string;
  admin: string;
  member: string;
  machines: string;
  machinesCount(n: number): string;
  state: string;
  online: string;
  offline: string;
  manage: string;
  leave: string;
  leaveTitle(network: string): string;
  leaveWhat(network: string, registered: boolean): string;
  leaveGo: string;
  gone: string;
}

export const zhNetwork: NetworkWords = {
  title: "网络",
  serverIs: "中枢服务器认出网络里的每台设备，管着谁能用哪台机器，机器之间的话经它转达",
  address: "中枢服务器地址",
  you: "你在这里是",
  admin: "管理员",
  member: "成员",
  machines: "机器",
  machinesCount: (n) => `${n} 台`,
  state: "连接",
  online: "已连接",
  offline: "未连接",
  manage: "管理中枢服务器",
  leave: "离开这个网络",
  leaveTitle: (network) => `离开 ${network}？`,
  leaveWhat: (network, registered) =>
    registered
      ? `这台电脑退出 ${network} 上的账号，不再是那里的一台机器，那里的人也就用不了它。要回来，重新加入就是。`
      : `这台设备退出 ${network} 上的账号。要回来，重新加入就是。`,
  leaveGo: "离开",
  gone: "这台设备已不在这个网络里",
};

export const enNetwork: NetworkWords = {
  title: "Network",
  serverIs: "The hub knows every device in the network, keeps who may use which machine, and passes on what the machines say to each other",
  address: "Hub address",
  you: "You are",
  admin: "Administrator",
  member: "Member",
  machines: "Machines",
  machinesCount: (n) => String(n),
  state: "Connection",
  online: "Connected",
  offline: "Not connected",
  manage: "Manage hub",
  leave: "Leave this network",
  leaveTitle: (network) => `Leave ${network}?`,
  leaveWhat: (network, registered) =>
    registered
      ? `This computer logs out of its accounts on ${network} and is no longer one of its machines, so nobody there can use it. Join again to come back.`
      : `This device logs out of its accounts on ${network}. Join again to come back.`,
  leaveGo: "Leave",
  gone: "This device is no longer in this network",
};

export interface NetworkPageOptions {
  store: Store;
  words: NetworkWords;
  hub: string;
  head(title: string, actions?: string): string;
  change(login: Login, made: AccountChange): Promise<void>;
  confirm: Confirm;
}

export class NetworkPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly body: HTMLElement;
  private readonly stop: () => void;

  constructor(private readonly options: NetworkPageOptions) {
    const { words } = options;
    const network = networksOf(options.store.whole.peek()).find((one) => one.hub === options.hub);
    this.title = network?.name ?? words.title;
    this.element = document.createElement("div");
    this.element.className = "network-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(this.title)}</h1>`)}<div class="network-body scroller"></div>`;
    this.body = this.element.querySelector<HTMLElement>(".network-body")!;
    this.body.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-leave]")) this.leave_();
    });
    this.stop = effect(() => {
      options.store.whole.get();
      this.draw();
    });
  }

  leave(): void {
    this.stop();
  }

  private draw(): void {
    const { words, store, hub } = this.options;
    const whole = store.whole.peek();
    const network = networksOf(whole).find((one) => one.hub === hub);
    if (!network) {
      this.body.innerHTML = `<p class="feature-note">${escape(words.gone)}</p>`;
      return;
    }
    const rows: [string, string][] = [
      [words.address, network.addresses.length ? network.addresses.map((address) => `<code>${escape(address)}</code>`).join(" ") : "—"],
      [words.you, `${escape(network.admin ? words.admin : words.member)} · ${network.logins.map(escape).join(", ")}`],
      [words.machines, `<a href="${link({ kind: "machines" })}">${escape(words.machinesCount(network.machines))}</a>`],
      [words.state, `<span class="network-dot${network.online ? " online" : ""}" aria-hidden="true"></span>${escape(network.online ? words.online : words.offline)}`],
    ];
    const html = `<figure class="network-figure">${serverDrawing()}<figcaption>${escape(words.serverIs)}</figcaption></figure>
      <dl class="network-facts">${rows.map(([name, value]) => `<div><dt>${escape(name)}</dt><dd>${value}</dd></div>`).join("")}</dl>
      <div class="network-acts">
        ${network.admin ? `<a class="text-button" href="${link({ kind: "admin" })}">${glyph("shield", 16)}<span>${escape(words.manage)}</span></a>` : ""}
        <button class="text-button danger-text" type="button" data-leave>${glyph("logout", 16)}<span>${escape(words.leave)}</span></button>
      </div>`;
    if (this.body.innerHTML !== html) this.body.innerHTML = html;
  }

  private leave_(): void {
    const { words, store, hub, confirm, change } = this.options;
    const whole = store.whole.peek();
    const network = networksOf(whole).find((one) => one.hub === hub);
    if (!network) return;
    const logins = loginsOf(whole?.accounts).filter((login) => login.hub === hub);
    const registered = whole?.accounts.hubs.find((one) => one.hub === hub)?.home !== null;
    confirm.ask({
      title: words.leaveTitle(network.name),
      what: words.leaveWhat(network.name, registered),
      go: words.leaveGo,
      act: async () => {
        // The account this computer is registered under goes last, so the
        // others are not left on a machine that has already gone.
        const home = whole?.accounts.hubs.find((one) => one.hub === hub)?.home ?? null;
        const ordered = [...logins.filter((login) => login.account !== home), ...logins.filter((login) => login.account === home)];
        for (const login of ordered) await change(login, { change: "logout" });
      },
      done: () => {
        void store.refresh();
        location.hash = link({ kind: "agents", machine: null });
      },
    });
  }
}

/** The server in the middle, three machines round it, each line drawn to
 *  it: the shape of a network, in pencil. */
function serverDrawing(): string {
  const random = seeded("network-server");
  const centre: Point = [120, 62];
  const around: Point[] = [
    [40, 34],
    [200, 36],
    [120, 128],
  ];
  const lines = around
    .map(([x, y]) => {
      const dx = centre[0] - x;
      const dy = centre[1] - y;
      const length = Math.hypot(dx, dy);
      const from: Point = [x + (dx / length) * 18, y + (dy / length) * 18];
      const to: Point = [centre[0] - (dx / length) * 24, centre[1] - (dy / length) * 24];
      return `<path class="nf-line" d="${ruled(from, to, 1.3, random)}"/>`;
    })
    .join("");
  const rings = around.map((at) => `<path class="nf-ring" d="${circle(at, 16, 1.4, random)}"/>`).join("");
  const marks = around
    .map(([x, y], index) => `<g transform="translate(${x - 9} ${y - 9})">${glyph(index === 2 ? "laptop" : index === 1 ? "server" : "desktop", 18)}</g>`)
    .join("");
  return `<svg class="network-drawing" viewBox="0 0 240 156" aria-hidden="true">
    <g filter="url(#pen-marker-lite)">${lines}${rings}<path class="nf-hub" d="${circle(centre, 22, 1.8, random)}"/></g>
    <g class="nf-mark">${marks}<g transform="translate(${centre[0] - 11} ${centre[1] - 11})">${glyph("server", 22)}</g></g>
  </svg>`;
}
