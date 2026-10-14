// The one page of a managed machine (`world-gui.md`, 受管机器上的界面): the
// whole window, with no sidebar. A drawing of this computer, its one line to
// the server and the lines of others' agents coming in; whose machine it is
// and on which network; whether it is connected; and, quietly in a corner,
// leaving the network and taking Fastctx off. A machine that no one signs
// in to is connected by its startup service alone: one missing is said,
// with the way to install it. Taken off the network by its owner, it says
// so, and comes back by a code.

import { escape } from "../conversation/prose";
import { names, type AccountsReply, type HubAccounts, type MemberSeen } from "../world/accounts";
import type { KeptNow } from "../world/bridge";
import type { Confirm } from "./confirm";
import { managedMachine } from "./managed-art";
import type { ManagedWords } from "./managed-words";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";

/** The network this computer is a managed machine on, and itself there;
 *  null when it is none's. */
export function managedOf(accounts: AccountsReply | null | undefined): { hub: HubAccounts; me: MemberSeen } | null {
  for (const hub of accounts?.hubs ?? []) {
    const here = hub.me ?? accounts!.machine;
    const me = hub.members.find((member) => names(member, here));
    if (me?.managed) return { hub, me };
  }
  return null;
}

export interface ManagedOptions {
  store: Store;
  words: ManagedWords;
  /** When, as the interface says it. */
  when(at: Date, now: Date): string;
  /** Leaves `hub`: this computer starts fresh after. */
  leave(hub: HubAccounts, owner: string): Promise<void>;
  /** Joining again, by a code. */
  rejoin(): void;
  /** Taking Fastctx off this computer, where it can be. */
  remove?: (() => void) | undefined;
  /** Whether the startup service is installed, and installing it, where
   *  this computer can say. */
  service?: { kept(): Promise<boolean>; keep(): Promise<KeptNow> } | undefined;
  confirm: Confirm;
}

export class ManagedPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly stop: () => void;
  private drawn = "";
  /** The startup service as last read: kept, missing, being installed, or
   *  to be installed by a command shown. */
  private service: { kind: "kept" | "missing" | "keeping" } | { kind: "command"; command: string } = { kind: "kept" };

  constructor(private readonly options: ManagedOptions) {
    this.title = options.words.title;
    this.element = document.createElement("div");
    this.element.className = "managed-page";
    this.element.addEventListener("click", (event) => this.click(event));
    this.stop = effect(() => this.draw(options.store.world.get()?.accounts, options.store.clock.get()));
    options.service?.kept().then(
      (kept) => this.serviceIs(kept ? { kind: "kept" } : { kind: "missing" }),
      (error: unknown) => console.error("reading whether the startup service is installed", error),
    );
  }

  private serviceIs(service: ManagedPage["service"]): void {
    this.service = service;
    this.drawn = "";
    this.draw(this.options.store.world.peek()?.accounts, this.options.store.clock.peek());
  }

  leave(): void {
    this.stop();
  }

  private draw(accounts: AccountsReply | null | undefined, now: Date): void {
    const { words, when } = this.options;
    const found = managedOf(accounts);
    if (!found) return;
    const { hub, me } = found;
    const owner = me.home ?? "";
    const link = hub.link;
    const removed = link.kind === "stopped";
    const state =
      link.kind === "online"
        ? { on: true, said: link.ms != null ? words.onlineMs(link.ms) : words.online }
        : link.kind === "connecting"
          ? { on: false, said: words.away(when(new Date(link.since), now)) }
          : null;
    const said = JSON.stringify([removed, owner, me.name, hub.name, state, link.kind === "stopped" ? link.reason : "", this.service]);
    if (said === this.drawn) return;
    this.drawn = said;
    this.element.classList.toggle("removed", removed);
    this.element.innerHTML = `
      <div class="managed">
        <div class="managed-drawing">${managedMachine()}</div>
        <h1 class="managed-title">${escape(removed ? words.removed : words.title)}</h1>
        <p class="managed-who">${escape(removed ? words.removedWhose(owner, hub.name) : words.whose(owner, me.name, hub.name))}</p>
        ${
          removed
            ? `<button class="sheet-go managed-rejoin" type="button" data-act="rejoin"${link.kind === "stopped" ? ` title="${escape(link.reason)}"` : ""}>${escape(words.rejoin)}</button>`
            : state
              ? `<p class="managed-link${state.on ? " on" : ""}"><span class="managed-dot" aria-hidden="true"></span>${escape(state.said)}</p>`
              : ""
        }
        ${removed ? "" : this.serviceSlip()}
      </div>
      <div class="managed-corner">
        <button class="text-button" type="button" data-act="leave">${escape(words.leave)}</button>
        ${this.options.remove ? `<button class="text-button" type="button" data-act="remove">${escape(words.uninstall)}</button>` : ""}
      </div>`;
  }

  /** The startup service missing: what that means here, and installing it. */
  private serviceSlip(): string {
    const { words } = this.options;
    const service = this.service;
    if (service.kind === "kept") return "";
    if (service.kind === "command") {
      return `<div class="managed-slip"><span>${escape(words.keepCommand)}</span><pre class="mono">${escape(service.command)}</pre></div>`;
    }
    const keeping = service.kind === "keeping";
    return `<div class="managed-slip"><span>${escape(words.unkept)}</span><button class="text-button" type="button" data-act="keep"${keeping ? " disabled" : ""}>${escape(keeping ? words.keeping : words.keep)}</button></div>`;
  }

  private click(event: Event): void {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]");
    if (!button) return;
    const { words, confirm, store } = this.options;
    switch (button.dataset.act) {
      case "keep": {
        const service = this.options.service;
        if (!service || this.service.kind === "keeping") return;
        this.serviceIs({ kind: "keeping" });
        void service.keep().then(
          (kept) =>
            this.serviceIs(
              kept.kind === "installed" || kept.kind === "already" ? { kind: "kept" } : kept.kind === "command" ? { kind: "command", command: kept.command } : { kind: "missing" },
            ),
          (error: unknown) => {
            console.error("installing the startup service", error);
            this.serviceIs({ kind: "missing" });
          },
        );
        return;
      }
      case "rejoin":
        return this.options.rejoin();
      case "remove":
        return this.options.remove?.();
      case "leave": {
        const found = managedOf(store.world.peek()?.accounts);
        if (!found) return;
        const { hub, me } = found;
        return confirm.ask({
          title: words.leaveTitle(hub.name),
          what: words.leaveWhat(hub.name),
          go: words.leaveGo,
          act: () => this.options.leave(hub, me.home ?? ""),
          done: () => void store.refresh(),
        });
      }
    }
  }
}
