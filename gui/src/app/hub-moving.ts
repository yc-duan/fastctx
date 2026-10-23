// Moving the server to another machine (`world-gui.md`, 把中枢服务器搬到别处).
// The administrator chooses, on the drawing, a machine of their own that
// can take it, or a new server over SSH; a red sheet names what goes with
// it and asks for the network's name. The server's details then follow the
// move step by step, ask for the service where it goes in, and, once it
// switched, say who has not followed yet, with the old place's forwarding
// stopped and its data deleted from there.

import { copyButton, copyFrom } from "./copy-button";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { HubAccounts, Login } from "../world/accounts";
import type { HubMove, HubMoveReply, HubMoveRequest, MoveStage, MoveTarget } from "../world/hub-move";
import type { AccessWords, LoginWords, MachinesWords } from "./account-words";
import { Sheet, type Confirm } from "./confirm";
import { RemoteWorld, type RemoteSeams } from "./remote-world";
import { stepMark as mark } from "../pen/step-mark";
import { holdUntilNamed, typedName } from "./typed-name";

/** The steps of a move, in order. */
const STAGES: MoveStage[] = ["preparing", "service", "reaching", "copying", "switching", "following", "done"];

/** How often the server's details ask how its move goes while they are
 *  open: a person is looking at it. */
const LOOK_MS = 2_000;

/** The port the server is reached at when it says none. */
const HUB_PORT = 7443;

export interface HubMovingOptions {
  words: MachinesWords;
  access: AccessWords;
  move(login: Login, request: HubMoveRequest): Promise<HubMoveReply>;
  /** Installs the moving server's service on this computer. */
  arrive(): Promise<void>;
  confirm: Confirm;
  /** The way over SSH, from a computer; a phone has none. */
  remote?: { seams: RemoteSeams; words: LoginWords; token(login: Login, machine: string): Promise<string> } | undefined;
  /** What is known changed: the page draws again. */
  changed(): void;
}

/** A machine that may take the server while one is being chosen. */
export interface Candidate {
  sleeps: boolean;
}

export class HubMoving {
  /** The moves of each hub, as last asked, by hub id. */
  private readonly moves = new Map<string, HubMove[]>();
  private watching: { hub: string; login: Login; timer: number } | null = null;
  /** The machines that may take the server, while one is chosen. */
  private choosing: { login: Login; hub: HubAccounts; targets: MoveTarget[]; port: number | null } | null = null;
  private readonly sheet = new Sheet("device-sheet move-sheet");
  private ssh: RemoteWorld | null = null;

  constructor(private readonly options: HubMovingOptions) {
    this.sheet.dialog.addEventListener("close", () => {
      this.ssh?.leave();
      this.ssh = null;
    });
  }

  dispose(): void {
    this.unwatch();
  }

  /** The server one is choosing a new place for, if one is. */
  get chosenHub(): string | null {
    return this.choosing?.hub.hub ?? null;
  }

  /** Begins choosing where `hub` moves: the machines that can take it light
   *  up. */
  async choose(login: Login, hub: HubAccounts): Promise<void> {
    const reply = await this.options.move(login, { request: "targets" });
    if (reply.kind !== "targets") throw new Error("the node answered something other than the machines");
    this.choosing = { login, hub, targets: reply.machines, port: reply.port };
    this.options.changed();
  }

  stopChoosing(): void {
    if (!this.choosing) return;
    this.choosing = null;
    this.options.changed();
  }

  /** The machines that can take the server now, by name. */
  candidates(): Map<string, Candidate> {
    const lit = new Map<string, Candidate>();
    for (const target of this.choosing?.targets ?? []) if (!target.unavailable && !target.hosts_hub) lit.set(target.name, { sleeps: target.sleeps });
    return lit;
  }

  /** Why `machine` cannot take the server, in the user's words; the
   *  server's own words behind it. */
  whyNot(machine: string): { said: string; detail: string } | null {
    const { words } = this.options;
    const target = this.choosing?.targets.find((one) => one.name === machine);
    if (!target) return { said: words.moveWhyNot.notOwn, detail: "" };
    if (target.hosts_hub) return { said: words.moveWhyNot.hosts, detail: "" };
    const why = target.unavailable;
    if (!why) return null;
    const moving = /^the hub is moving to (\S+) now$/.exec(why);
    const said =
      why === "it is offline"
        ? words.moveWhyNot.offline
        : why === "the hub runs there now"
          ? words.moveWhyNot.hosts
          : moving
            ? words.moveWhyNot.moving(moving[1]!)
            : why.startsWith("it runs a fastctx that cannot take the hub")
              ? words.moveWhyNot.older
              : why === "the hub runs on Windows and Linux"
                ? words.moveWhyNot.system
                : words.moveWhyNot.other;
    return { said, detail: why };
  }

  /** The red sheet that moves the server to `machine`, one of the lit. */
  confirmTo(machine: string, done: () => void): void {
    const choosing = this.choosing;
    const target = choosing?.targets.find((one) => one.name === machine);
    if (!choosing || !target) return;
    const { words } = this.options;
    const seen = target.seen ? [hostOf(target.seen)] : [];
    const cards = (target.interfaces ?? []).map((one) => one.address).filter((address) => !seen.includes(address));
    const choice = (host: string, note: string, checked: boolean) =>
      `<label class="choice map-card-choice"><input type="checkbox" name="host" value="${escape(host)}"${checked ? " checked" : ""}><span class="mono">${escape(host)}</span><span class="choice-note">${escape(note)}</span></label>`;
    const confirm = this.options.confirm;
    confirm.ask({
      title: words.moveTitle(machine),
      what: words.moveSaid,
      fields: `${this.carries()}
        ${target.sleeps ? `<p class="move-sleeps">${glyph("sleep", 14)}<span>${escape(words.moveSleeps)}</span></p>` : ""}
        <fieldset class="map-card-field"><legend>${escape(words.directAddresses)}</legend><div class="choice-options">${[
          ...seen.map((host) => choice(host, words.directSeen, true)),
          ...cards.map((host) => choice(host, words.directCard, !seen.length)),
        ].join("")}</div></fieldset>
        <label class="field map-card-port"><span class="field-label">${escape(words.directPort)}</span><input class="mono" name="port" inputmode="numeric" value="${choosing.port ?? HUB_PORT}" autocomplete="off" spellcheck="false" required></label>
        ${typedName(words.typeName(choosing.hub.name))}`,
      go: words.moveGo,
      act: async () => {
        const body = confirm.body;
        const hosts = [...body.querySelectorAll<HTMLInputElement>("input[name=host]:checked")].map((input) => input.value);
        const port = Number(body.querySelector<HTMLInputElement>("input[name=port]")!.value.trim());
        if (!hosts.length) throw new Error(words.directPickOne);
        if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error(words.directPortWrong);
        await this.options.move(choosing.login, { request: "start", to: machine, addresses: hosts.map((host) => withPort(host, port)) });
      },
      done: () => {
        this.choosing = null;
        this.watch(choosing.login, choosing.hub.hub, true);
        done();
      },
    });
    holdUntilNamed(confirm.body, choosing.hub.name);
  }

  /** The sheet that moves the server to a new server: the same red words,
   *  then the server is added over SSH and takes it. */
  toNewServer(done: () => void): void {
    const choosing = this.choosing;
    const remote = this.options.remote;
    if (!choosing || !remote) return;
    this.choosing = null;
    this.options.changed();
    const { words } = this.options;
    const { sheet } = this;
    sheet.body.innerHTML = `${sheet.head(words.moveNewTitle, this.options.access.close)}
      <p class="confirm-what">${escape(words.moveSaid)}</p>
      ${this.carries()}
      ${typedName(words.typeName(choosing.hub.name))}
      <footer class="sheet-foot"><span></span><button class="sheet-go danger" type="submit">${escape(words.moveNext)}</button></footer>`;
    holdUntilNamed(sheet.body, choosing.hub.name);
    sheet.body.onsubmit = (event) => {
      event.preventDefault();
      if (this.ssh) return this.ssh.submit();
      sheet.body.innerHTML = `${sheet.head(words.moveNewTitle, this.options.access.close)}<div class="device-remote"></div>`;
      this.ssh = new RemoteWorld(sheet.body.querySelector<HTMLElement>(".device-remote")!, {
        words: remote.words,
        seams: remote.seams,
        back: this.options.access.cancel,
        adding: {
          token: (machine) => remote.token(choosing.login, machine),
          then: async (session, machine, host, step) => {
            const reply = await this.options.move(choosing.login, { request: "targets" });
            const target = reply.kind === "targets" ? reply.machines.find((one) => one.name === machine) : undefined;
            const port = (reply.kind === "targets" ? reply.port : null) ?? HUB_PORT;
            // Where the server sees it come from is where others reach it;
            // the address it was reached at over SSH, when that is one.
            const hosts = [...new Set([...(target?.seen ? [hostOf(target.seen)] : []), ...(isAddress(host) ? [host] : [])])];
            if (!hosts.length) throw new Error(target?.unavailable ?? `${machine} is not among the machines that can take the hub`);
            await this.options.move(choosing.login, { request: "start", to: machine, addresses: hosts.map((one) => withPort(one, port)) });
            await remote.seams.arrive(session, step);
          },
          added: () => {
            this.ssh?.leave();
            this.ssh = null;
            sheet.close();
            this.watch(choosing.login, choosing.hub.hub, true);
            done();
          },
        },
      });
    };
    sheet.show();
  }

  /** Asks how `hub`'s move goes while its details are open. */
  watch(login: Login, hub: string, now = false): void {
    if (this.watching?.hub === hub && !now) return;
    this.unwatch();
    const look = () => {
      this.options.move(login, { request: "status" }).then(
        (reply) => {
          if (reply.kind !== "moves") return;
          const before = JSON.stringify(this.moves.get(hub) ?? []);
          this.moves.set(hub, reply.moves);
          if (JSON.stringify(reply.moves) !== before) this.options.changed();
        },
        (error: unknown) => console.warn("the hub's move could not be read", error),
      );
    };
    look();
    this.watching = { hub, login, timer: window.setInterval(look, LOOK_MS) };
  }

  unwatch(): void {
    if (this.watching) clearInterval(this.watching.timer);
    this.watching = null;
  }

  /** The move under way, or the last one, of `hub`. */
  current(hub: string): HubMove | null {
    const moves = this.moves.get(hub) ?? [];
    return moves.find((one) => one.stage !== "done" && one.stage !== "cancelled") ?? moves.at(-1) ?? null;
  }

  /** Under the server's name on the drawing while it moves. */
  label(hub: string): string | null {
    const move = this.current(hub);
    const { words } = this.options;
    if (!move || move.stage === "cancelled") return null;
    if (move.stage === "following" && move.following) {
      const { machines, devices } = move.following;
      return machines.length || devices.length ? words.moveFollowing(machines.length, devices.length) : words.moveAllFollowed;
    }
    return move.stage === "done" ? null : words.moveTo(move.to);
  }

  /** The move in the server's details: each step ticked, what waits for
   *  someone, who has not followed, and what is left at the old place. */
  section(hub: string, here: string | null): string {
    const move = this.current(hub);
    if (!move) return "";
    const { words } = this.options;
    const reached = STAGES.indexOf(move.stage);
    const steps =
      move.stage === "cancelled"
        ? `<p class="panel-said">${escape(words.moveStages.cancelled)}</p>`
        : `<ol class="move-steps">${STAGES.slice(0, -1)
            .map((stage, index) => {
              const state = index < reached || move.stage === "done" ? "done" : index === reached && move.error ? "failed" : null;
              return `<li class="${index === reached && move.stage !== "done" ? "now" : ""}"><span class="removal-mark" aria-hidden="true">${mark(state)}</span><span>${escape(words.moveStages[stage])}</span></li>`;
            })
            .join("")}</ol>`;
    const error = move.error ? `<p class="move-error" title="${escape(move.error)}">${escape(this.options.access.failed(move.error))}</p>` : "";
    const waiting =
      move.waiting?.kind === "service" && !move.error
        ? move.to === here
          ? `<p class="panel-said">${escape(words.moveInstallHere)}</p><div class="move-acts">${button("arrive", words.moveInstall)}</div>`
          : `<p class="panel-said">${escape(words.moveInstallThere(move.to))}</p><p class="move-command"><code class="mono">${escape(words.moveInstallCommand(move.waiting.os))}</code>${copyButton(words.moveInstallCommand(move.waiting.os), words.moveCopy, `data-move-act="copy"`)}</p>`
        : "";
    const following = move.following;
    const old = following
      ? `<p class="panel-said"${following.machines.length || following.devices.length ? ` title="${escape([...following.machines, ...following.devices].join("、"))}"` : ""}>${escape(
          following.machines.length || following.devices.length ? words.moveFollowing(following.machines.length, following.devices.length) : words.moveAllFollowed,
        )}</p>
        <p class="panel-said">${escape(
          following.old.state === "forwarding"
            ? words.moveOldForwards
            : following.old.state === "deleted"
              ? words.moveOldDeleted
              : words.moveOldStopped,
        )}</p>`
      : "";
    const acts = [
      reached < STAGES.indexOf("switching") && move.stage !== "cancelled" ? button("call", words.moveCall) : "",
      move.error ? button("retry", words.moveRetry) : "",
      following?.old.state === "forwarding" ? button("stop", words.moveStop, "data-danger") : "",
      following?.old.state === "stopped" ? button("delete", words.moveDelete, "data-danger") : "",
    ].join("");
    return `<section class="move-section" data-move="${escape(move.id)}">
      <h3 class="move-title">${glyph("move", 14)}<span>${escape(move.stage === "done" || move.stage === "following" ? words.moveMoved(move.to) : words.moveTo(move.to))}</span></h3>
      ${steps}${error}${waiting}${old}
      ${acts ? `<div class="move-acts">${acts}</div>` : ""}
    </section>`;
  }

  /** A button pressed in the move's section of `hub`'s details. */
  async click(target: HTMLElement, hub: string): Promise<void> {
    const button = target.closest<HTMLButtonElement>("[data-move-act]");
    const watching = this.watching;
    const move = this.current(hub);
    if (!button || !watching || watching.hub !== hub || !move) return;
    const { words } = this.options;
    const ask = (request: HubMoveRequest) => this.options.move(watching.login, request).then(() => this.watch(watching.login, hub, true));
    switch (button.dataset.moveAct) {
      case "arrive":
        button.disabled = true;
        await this.options.arrive().finally(() => (button.disabled = false));
        this.watch(watching.login, hub, true);
        return;
      case "copy":
        return copyFrom(button, words.moveCopied);
      case "call":
        return ask({ request: "cancel" });
      case "retry":
        return ask({ request: "retry" });
      case "stop":
        this.options.confirm.ask({
          title: words.moveStopTitle,
          what: words.moveStopSaid([...(move.following?.machines ?? []), ...(move.following?.devices ?? [])]),
          go: words.moveStop,
          act: () => ask({ request: "stop_forwarding", id: move.id }),
          done: () => this.options.changed(),
        });
        return;
      case "delete":
        this.options.confirm.ask({
          title: words.moveDeleteTitle,
          what: words.moveDeleteSaid,
          go: words.moveDelete,
          act: () => ask({ request: "delete_old", id: move.id }),
          done: () => this.options.changed(),
        });
        return;
    }
  }

  /** What goes with the server, as a row of tags. */
  private carries(): string {
    const { words } = this.options;
    return `<p class="move-carries">${words.moveCarries.map((one) => `<span class="tag">${escape(one)}</span>`).join("")}</p>
      <details class="map-card-more"><summary>${escape(words.more)}</summary><p>${escape(words.moveWhat)}</p></details>`;
  }
}

function button(act: string, label: string, extra = ""): string {
  return `<button class="text-button quiet-button" type="button" data-move-act="${act}"${extra ? ` ${extra}` : ""}><span>${escape(label)}</span></button>`;
}

/** The host of an address that may carry a port. */
function hostOf(address: string): string {
  const bracketed = /^\[([^\]]+)\](?::\d+)?$/.exec(address);
  if (bracketed) return bracketed[1]!;
  return /^[^:]+:\d+$/.test(address) ? address.slice(0, address.lastIndexOf(":")) : address;
}

function withPort(host: string, port: number): string {
  return host.includes(":") ? `[${host}]:${port}` : `${host}:${port}`;
}

/** Whether `host` is an IP address rather than a name. */
function isAddress(host: string): boolean {
  return /^\d{1,3}(\.\d{1,3}){3}$/.test(host) || host.includes(":");
}
