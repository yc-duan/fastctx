// The small cards over the machines page's drawing (`machines.ts`) that ask
// for one thing: a machine's name or group, a group's name, and letting
// other machines connect to one directly or through it, or no longer. Each
// stands beside its ring, or rises from below on a narrow page; a change
// made by one can be undone for a moment from the drawing's corner. The
// people's cards (`map-people.ts`) open here too.

import { escape } from "../conversation/prose";
import type { Point } from "../map-canvas";
import { blot, circle, outline, ruled, seeded } from "../pen/shapes";
import type { AccountChange, Login, MemberSeen } from "../world/accounts";
import type { NameProblem, WorldNow } from "../world/bridge";
import type { AccessWords, MachinesWords } from "./account-words";
import { layerOpened, type Layer } from "./back";
import { chosenGroup, groupChange, groupForm, groupNameOf, groupRenameForm, regroup, renameGroups, wireGroupForm, type GroupBox, type Grouping } from "./machine-groups";
import type { MachinePageWords } from "./machine-page-words";
import type { NameSaid } from "./name-words";
import type { Machine } from "./reach";
import { RenameField, renamerOf } from "./rename";

/** Renaming a machine: the words of its field, the rule a name is checked
 *  by, and renaming this computer. */
export interface Renaming {
  words: MachinePageWords;
  nameSaid: NameSaid;
  check(text: string): Promise<NameProblem | null>;
  here(to: string): Promise<void>;
}

export interface CardsHost {
  words: MachinesWords;
  access: AccessWords;
  renaming: Renaming;
  change(login: Login, change: AccountChange): Promise<void>;
  refresh(): Promise<void>;
  world(): WorldNow | null;
  /** Where a card about `key` stands beside, on the drawing. */
  anchorOf(key: string): Point | null;
  /** How far right and down on the drawing a card may reach. */
  room(): { right: number; bottom: number };
  /** Whether the page is narrow, where a card rises from below. */
  narrow(): boolean;
  /** A card opened: nothing else floats over the drawing, and the page is
   *  drawn again. */
  opened(): void;
  /** The card closed, `held` whether the keys were in it. */
  closed(held: boolean): void;
  redraw(): void;
  /** A change failed where the card has no line to say it. */
  failed(key: string, said: string): void;
  /** A change made, undone for a moment from the drawing's corner. */
  undo(said: string, redo: () => Promise<void>): void;
  /** Machines circled for a moment where they stand anew, by their keys;
   *  `chosen` is chosen as well. */
  arrived(keys: string[], chosen?: string): void;
  /** Whether the hub is checking that it reaches a machine directly. */
  checking(key: string, on: boolean): void;
}

/** The port offered first for direct connections: the one after the hub's
 *  own, 7443. */
const DIRECT_PORT = 7444;

export class MachineCards {
  /** The spot the open card is about. */
  private about: string | null = null;
  /** The card as the system back closes it. */
  private readonly layer: Layer;

  constructor(
    readonly element: HTMLElement,
    private readonly host: CardsHost,
  ) {
    this.layer = {
      isOpen: () => !!this.about && element.isConnected,
      close: () => this.close(),
      holds: (target) => element.contains(target),
    };
    element.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-card=cancel]")) this.close();
    });
  }

  /** The spot the open card is about, or none. */
  get open(): string | null {
    return this.about;
  }

  /** The card that puts machines in a group, beside the ring of the one, or
   *  the first of those framed together: one of their owner's groups, a new
   *  one, or none. Put there, each is circled in its new box, and for a
   *  moment the change can be undone from the drawing's corner. */
  group(key: string, machines: Machine[], groupings: Grouping[]): void {
    const words = this.host.words.groups;
    const first = groupings[0];
    if (!first) return;
    const now = groupings.every((one) => one.now === first.now) ? first.now : undefined;
    const title = machines.length === 1 ? words.groupTitle(machines[0]!.written) : words.groupMany(machines.length);
    this.show(key, groupForm(words, title, first.groups, now), async () => {
      const chosen = chosenGroup(this.element, words);
      if ("wrong" in chosen) throw this.wrong(chosen.wrong);
      const moving = machines.filter((_, at) => groupings[at]!.now !== chosen.group);
      if (!moving.length) return;
      const before = new Map(machines.map((machine, at) => [machine.name, groupings[at]!.now]));
      for (const machine of moving) await this.host.change(first.login, groupChange(machine.name, chosen.group));
      this.circle(moving);
      this.host.undo(machines.length === 1 ? words.movedOne(machines[0]!.written, chosen.group) : words.movedMany(moving.length, chosen.group), async () => {
        for (const machine of moving) await this.host.change(first.login, groupChange(machine.name, before.get(machine.name) ?? null));
      });
    });
    wireGroupForm(this.element);
  }

  /** The card that renames a group: its machines are in the group of the
   *  new name, with any already in one of that name. */
  renameGroup(key: string, box: GroupBox): void {
    const words = this.host.words.groups;
    const change = (login: Login, change: AccountChange) => this.host.change(login, change);
    this.show(key, groupRenameForm(words, box.group), async () => {
      const named = groupNameOf(this.element.querySelector<HTMLInputElement>("input[name=name]")?.value ?? "", words);
      if ("wrong" in named) throw this.wrong(named.wrong);
      if (named.name === box.group) return;
      await renameGroups(box, named.name, change);
      this.circle(box.machines);
      this.host.undo(words.renamedGroup(box.group, named.name), () => regroup(box, change));
    });
    this.element.querySelector<HTMLInputElement>("input[name=name]")?.select();
  }

  /** A card beside the ring with its name to change. Renamed, it is
   *  circled under its new name, and for a moment the old name can be given
   *  back from the drawing's corner: a name a machine had stays its own. */
  rename(key: string, machine: Machine): void {
    const world = this.host.world();
    const rename = world ? renamerOf(machine, world, (login, change) => this.host.change(login, change), this.host.renaming.here) : null;
    if (!rename) return;
    const { words, nameSaid, check } = this.host.renaming;
    const from = machine.name;
    this.show(key, `<p class="map-card-title">${escape(words.renameTitle(machine.written))}</p><div class="map-card-rename"></div>`, async () => {});
    const field = new RenameField({
      words,
      nameSaid,
      name: from,
      note: !machine.here && machine.online === false ? words.renameOffline(machine.written) : undefined,
      check,
      rename: async (to) => {
        await rename(to);
        await this.host.refresh();
      },
      done: (to) => {
        field.dispose();
        this.close();
        this.host.arrived([`m:${to}`], `m:${to}`);
        this.host.undo(words.renamed(from, to), () => rename(from, to));
        this.host.redraw();
      },
      cancel: () => {
        field.dispose();
        this.close();
      },
    });
    this.element.querySelector(".map-card-rename")!.append(field.element);
    field.focus();
  }

  /** The card that lets other machines connect to one directly: a small
   *  drawing of their lines meeting it, its addresses to choose from (its
   *  network cards' and where the server sees it, never an example), and
   *  the port. The server then tries it from outside; until it has, the
   *  socket on its ring is pencil, and what kept the server out is said in
   *  the card. */
  direct(login: Login, machine: Machine, key: string, member: MemberSeen): void {
    const { words } = this.host;
    const seen = member.reach?.seen ? [hostOf(member.reach.seen)] : [];
    const cards = (member.reach?.interfaces ?? []).map(hostOf).filter((host) => !seen.includes(host));
    const choice = (host: string, note: string, checked: boolean) =>
      `<label class="choice map-card-choice"><input type="checkbox" name="host" value="${escape(host)}"${checked ? " checked" : ""}><span class="mono">${escape(host)}</span><span class="choice-note">${escape(note)}</span></label>`;
    this.show(
      key,
      `<p class="map-card-title">${escape(words.directTitle(machine.written))}</p>
       <div class="map-card-pictures">${directPicture(true)}</div>
       <fieldset class="map-card-field"><legend>${escape(words.directAddresses)}</legend><div class="choice-options">${[
         ...seen.map((host) => choice(host, words.directSeen, true)),
         ...cards.map((host) => choice(host, words.directCard, !seen.length)),
       ].join("")}</div></fieldset>
       <label class="field map-card-port"><span class="field-label">${escape(words.directPort)}</span><input class="mono" name="port" inputmode="numeric" value="${DIRECT_PORT}" autocomplete="off" spellcheck="false" required></label>
       <p class="map-card-wrong" role="alert" hidden></p>
       <details class="map-card-more"><summary>${escape(words.more)}</summary><p>${escape(words.directWhat)}</p></details>
       <footer class="map-card-acts"><button class="text-button" type="button" data-card="cancel">${escape(words.cancel)}</button><button class="text-button map-card-go" type="submit">${escape(words.directGo)}</button></footer>`,
      async () => {
        const { hosts, port } = this.addresses();
        const wrong = this.element.querySelector<HTMLElement>(".map-card-wrong")!;
        wrong.textContent = words.directChecking;
        wrong.hidden = false;
        wrong.classList.add("quiet");
        this.host.checking(key, true);
        try {
          await this.host.change(login, { change: "direct", machine: machine.name, at: hosts.map((host) => withPort(host, port)) });
        } catch (error) {
          wrong.classList.remove("quiet");
          throw this.wrong(error instanceof Error ? error.message : String(error));
        } finally {
          this.host.checking(key, false);
        }
      },
    );
  }

  /** Takes direct connections to a machine away at once, after a small
   *  card of before and after; for a moment they can be given back. */
  stopDirect(login: Login, machine: Machine, key: string, before: string[]): void {
    const { words } = this.host;
    this.show(
      key,
      `<p class="map-card-title">${escape(words.stopDirectTitle(machine.written))}</p>
       <div class="map-card-pictures">${directPicture(true)}<span aria-hidden="true">→</span>${directPicture(false)}</div>
       <p class="map-card-said">${escape(words.stopDirectSaid)}</p>
       <p class="map-card-wrong" role="alert" hidden></p>
       <footer class="map-card-acts"><button class="text-button" type="button" data-card="cancel">${escape(words.cancel)}</button><button class="text-button map-card-go" type="submit">${escape(words.confirm)}</button></footer>`,
      async () => {
        await this.host.change(login, { change: "direct", machine: machine.name, at: [] });
        this.host.undo(words.directStopped(machine.written), () => this.host.change(login, { change: "direct", machine: machine.name, at: before }));
      },
    );
  }

  /** The card that lets one's own computer relay for one's other machines
   *  on its network: its network cards' addresses to choose from, and the
   *  port. Nothing checks it from outside: only its own network reaches it. */
  relay(login: Login, machine: Machine, key: string, member: MemberSeen): void {
    const { words } = this.host;
    // An IPv6 link-local address is named by each machine's own card, so no
    // other machine reaches the relay at it.
    const cards = (member.reach?.interfaces ?? []).map(hostOf).filter((host) => !/^fe[89ab][0-9a-f]:/i.test(host));
    this.show(
      key,
      `<p class="map-card-title">${escape(words.relayTitle(machine.written))}</p>
       <fieldset class="map-card-field"><legend>${escape(words.relayAddresses)}</legend><div class="choice-options">${cards
         .map((host, index) => `<label class="choice map-card-choice"><input type="checkbox" name="host" value="${escape(host)}"${index === 0 ? " checked" : ""}><span class="mono">${escape(host)}</span><span class="choice-note">${escape(words.directCard)}</span></label>`)
         .join("")}</div></fieldset>
       <label class="field map-card-port"><span class="field-label">${escape(words.directPort)}</span><input class="mono" name="port" inputmode="numeric" value="${DIRECT_PORT}" autocomplete="off" spellcheck="false" required></label>
       <p class="map-card-wrong" role="alert" hidden></p>
       <details class="map-card-more"><summary>${escape(words.more)}</summary><p>${escape(words.relayWhat)}</p></details>
       <footer class="map-card-acts"><button class="text-button" type="button" data-card="cancel">${escape(words.cancel)}</button><button class="text-button map-card-go" type="submit">${escape(words.relayGo)}</button></footer>`,
      async () => {
        const { hosts, port } = this.addresses();
        try {
          await this.host.change(login, { change: "relay", machine: machine.name, at: hosts.map((host) => withPort(host, port)) });
        } catch (error) {
          throw this.wrong(error instanceof Error ? error.message : String(error));
        }
      },
    );
  }

  /** Stops one's own computer relaying at once; for a moment it can be
   *  turned on again where it was. */
  async stopRelay(login: Login, machine: Machine, before: string[]): Promise<void> {
    await this.host.change(login, { change: "relay", machine: machine.name, at: [] });
    this.host.undo(this.host.words.relayStopped(machine.written), () => this.host.change(login, { change: "relay", machine: machine.name, at: before }));
  }

  /** A card beside the ring of `key`, or rising from below on a narrow
   *  page; `go` runs when it is confirmed, and an error it throws stays in
   *  the card. */
  show(key: string, html: string, go: () => Promise<void | "keep">): HTMLElement {
    const card = this.element;
    const at = this.host.anchorOf(key) ?? [0, 0];
    this.about = key;
    this.host.opened();
    card.innerHTML = html;
    const sheet = this.host.narrow();
    card.classList.toggle("as-sheet", sheet);
    card.hidden = false;
    layerOpened(this.layer);
    if (!sheet) {
      // Beside the ring, on its right unless what is open on the right, the
      // panel or the canvas's edge, leaves no room there.
      const { right, bottom } = this.host.room();
      const width = card.offsetWidth;
      const x = at[0] + 16 + width + 8 <= right ? at[0] + 16 : at[0] - 16 - width;
      const y = Math.min(bottom - card.offsetHeight - 8, at[1] - 24);
      card.style.transform = `translate(${Math.max(8, Math.min(x, right - width - 8))}px, ${Math.max(8, y)}px)`;
    } else card.style.transform = "";
    card.onsubmit = (event) => {
      event.preventDefault();
      const button = card.querySelector<HTMLButtonElement>(".map-card-go");
      if (button) button.disabled = true;
      go().then(
        (kept) => {
          if (kept === "keep") {
            if (button) button.disabled = false;
            return;
          }
          this.close();
          void this.host.refresh().then(() => this.host.redraw());
        },
        (error: unknown) => {
          if (button) button.disabled = false;
          if (error instanceof Error && error.message === "kept open") return;
          const said = error instanceof Error ? error.message : String(error);
          const wrong = card.querySelector<HTMLElement>(".map-card-wrong");
          if (wrong) {
            wrong.textContent = said;
            wrong.hidden = false;
          } else this.host.failed(key, said);
        },
      );
    };
    card.focus({ preventScroll: true });
    return card;
  }

  close(): void {
    if (!this.about) return;
    const card = this.element;
    const held = card.contains(document.activeElement);
    this.about = null;
    card.hidden = true;
    card.innerHTML = "";
    card.onsubmit = null;
    this.host.closed(held);
  }

  /** Says in the open card why it does not go, and keeps it open. */
  private wrong(said: string): Error {
    const wrong = this.element.querySelector<HTMLElement>(".map-card-wrong");
    if (wrong) {
      wrong.textContent = said;
      wrong.hidden = false;
    }
    return new Error("kept open");
  }

  /** The addresses ticked in the card and the port written, or why they
   *  do not go. */
  private addresses(): { hosts: string[]; port: number } {
    const { words } = this.host;
    const hosts = [...this.element.querySelectorAll<HTMLInputElement>("input[name=host]:checked")].map((input) => input.value);
    const port = Number(this.element.querySelector<HTMLInputElement>("input[name=port]")!.value.trim());
    if (!hosts.length) throw this.wrong(words.directPickOne);
    if (!Number.isInteger(port) || port < 1 || port > 65535) throw this.wrong(words.directPortWrong);
    return { hosts, port };
  }

  private circle(machines: Machine[]): void {
    this.host.arrived(machines.map((machine) => `m:${machine.written}`));
  }
}

/** An address as the hub or a network card gives it, without a port. */
function hostOf(address: string): string {
  const bracketed = /^\[([^\]]+)\](?::\d+)?$/.exec(address);
  if (bracketed) return bracketed[1]!;
  return /^[^:]+:\d+$/.test(address) ? address.slice(0, address.lastIndexOf(":")) : address;
}

/** A host and a port as one address, an IPv6 host in brackets. */
function withPort(host: string, port: number): string {
  return host.includes(":") ? `[${host}]:${port}` : `${host}:${port}`;
}

/** A machine others connect to directly, drawn small for a card: its ring
 *  in the middle with its line to the hub's knot above, and two machines
 *  beside it with theirs; with direct connections, straight clay lines from
 *  the two to it. */
function directPicture(direct: boolean): string {
  const random = seeded(`direct-picture:${direct}`);
  const knot: [number, number] = [66, 7];
  const middle: [number, number] = [66, 30];
  const sides: [number, number][] = [
    [18, 26],
    [114, 26],
  ];
  const rings = [circle(middle, 8, 1.5, random), ...sides.map((side) => circle(side, 6, 1.3, random))];
  const bowed = (from: [number, number], reach: number) => {
    const dx = knot[0] - from[0];
    const dy = knot[1] - from[1];
    const length = Math.hypot(dx, dy);
    const start: [number, number] = [from[0] + (dx / length) * (reach + 2), from[1] + (dy / length) * (reach + 2)];
    const end: [number, number] = [knot[0] - (dx / length) * 5, knot[1] - (dy / length) * 5];
    return outline([start, [(start[0] + end[0]) / 2 + 2, (start[1] + end[1]) / 2 - 2], end], 1.1, random, { pressure: 0.3, taper: 0.1 });
  };
  const lines = [bowed(middle, 8), ...sides.map((side) => bowed(side, 6))];
  const straight = direct ? sides.map((side) => ruled([side[0] + (side[0] < 66 ? 8 : -8), side[1] + 1], [middle[0] + (side[0] < 66 ? -10 : 10), middle[1]], 1.1, random)) : [];
  return `<svg class="map-card-picture" viewBox="0 0 132 40" aria-hidden="true"><g class="ink graphite" filter="url(#pen-marker)"><path d="${circle(knot, 3.5, 1.2, random)}"/><path d="${blot(knot, 1.5, random)}"/>${[...rings, ...lines].map((d) => `<path d="${d}"/>`).join("")}</g><g class="ink clay" filter="url(#pen-marker)">${straight.map((d) => `<path d="${d}"/>`).join("")}</g></svg>`;
}
