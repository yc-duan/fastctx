// Where a member hangs in the World's tree (`world-network.md`, 一个中枢，一棵树),
// set from a machine with management rights: the relay it connects
// through first and the one it falls back to, or straight to the hub; and
// whether a machine relays for the machines below it, at which addresses.
// The hub is always the last way up, so nothing set here can leave a
// machine with none. The World refuses what this machine may not sign, and
// the sheet stays open with why.

import { escape } from "../conversation/prose";
import type { Node, Place } from "../world/world";
import { Sheet } from "./confirm";
import type { TreeWords } from "./words";

/** The relays a member connects through, in order, as its place says. */
export function relaysOf(place: Place | null | undefined): string[] {
  return (place?.via ?? []).flatMap((upstream) => (upstream.kind === "relay" ? [upstream.machine] : []));
}

/** Where a member hangs, in a sentence; empty when it is straight below
 *  the hub and relays for nobody, which is how every member starts. */
export function placeLine(node: Node, words: TreeWords): string {
  const relays = relaysOf(node.place);
  const parts: string[] = [];
  if (relays.length) parts.push(words.through(relays[0], relays.slice(1)));
  if (node.place?.relays_at?.length) parts.push(words.relaysAt(node.place.relays_at));
  return parts.join(" · ");
}

/** The World's tree: the hub at the root, each member below the relay its
 *  route leads through first, while that relay relays. Drawn only once
 *  some machine relays; until then every member is straight below the
 *  hub, and the list says it. Each name brings its entry into view. */
export function structure(nodes: Node[], here: string, words: TreeWords): string {
  const members = nodes.filter((node) => !node.revoked);
  const relays = new Set(members.filter((node) => node.place?.relays_at?.length).map((node) => node.name));
  if (!relays.size) return "";
  const parent = (node: Node) => relaysOf(node.place).find((relay) => relays.has(relay) && relay !== node.name) ?? null;
  const below = (relay: string | null, seen: Set<string>): string => {
    const children = members.filter((node) => parent(node) === relay && !seen.has(node.name));
    if (!children.length) return "";
    return `<ul>${children
      .map((node) => {
        const inner = relays.has(node.name) ? below(node.name, new Set([...seen, node.name])) : "";
        const tags = [node.name === here ? words.here : "", relays.has(node.name) ? words.relayTag : ""].filter(Boolean);
        return `<li><button type="button" class="tree-node" data-show="${escape(node.name)}">${escape(node.name)}</button>${tags.map((tag) => `<span class="tree-tag">${escape(tag)}</span>`).join("")}${inner}</li>`;
      })
      .join("")}</ul>`;
  };
  return `<section class="machines-tree" aria-label="${escape(words.structure)}"><h2 class="machines-tree-title">${escape(words.structure)}</h2><ul class="tree-root"><li><span class="tree-hub">${escape(words.hub)}</span>${below(null, new Set())}</li></ul></section>`;
}

export class RouteSheet {
  private readonly sheet = new Sheet("device-sheet tree-sheet");

  constructor(private readonly words: TreeWords) {}

  /** Opens the sheet for `member`; `relays` are the machines that relay
   *  now; `route` signs what is chosen. */
  open(member: Node, relays: string[], route: (via: string[]) => Promise<void>, done: () => void): void {
    const { sheet, words } = this;
    const offered = relays.filter((relay) => relay !== member.name);
    const now = relaysOf(member.place);
    let first = now[0] ?? "";
    let then = now[1] ?? "";
    const chips = (attribute: string, items: string[], chosen: string, title: (item: string) => string) =>
      `<div class="choice-options" role="radiogroup">${items
        .map(
          (item) =>
            `<button type="button" class="choice${item === chosen ? " chosen" : ""}" role="radio" aria-checked="${item === chosen}" ${attribute}="${escape(item)}">${escape(title(item))}</button>`,
        )
        .join("")}</div>`;
    const draw = () => {
      const fallbacks = offered.filter((relay) => relay !== first);
      if (!first || !fallbacks.includes(then)) then = "";
      sheet.body.innerHTML = `
        ${sheet.head(words.routeTitle(member.name), words.close)}
        <p class="confirm-what">${escape(words.routeWhat)}</p>
        ${
          offered.length
            ? `<div class="field"><span class="field-label">${escape(words.first)}</span>
                ${chips("data-first", ["", ...offered], first, (item) => (item ? words.relay(item) : words.straight))}</div>
              ${
                first && fallbacks.length
                  ? `<div class="field"><span class="field-label">${escape(words.fallback)}</span>
                      ${chips("data-then", ["", ...fallbacks], then, (item) => (item ? words.relay(item) : words.hubLast))}</div>`
                  : ""
              }`
            : `<p class="field-hint">${escape(words.noRelays)}</p>`
        }
        <p class="field-hint tree-says" aria-live="polite">${escape(first ? words.through(first, then ? [then] : []) + words.thenHub : words.straightSaid)}</p>
        <p class="sheet-note" role="alert" hidden></p>
        <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit"${offered.length || now.length ? "" : " disabled"}>${escape(words.routeGo)}</button></footer>`;
    };
    draw();
    sheet.body.onclick = (event) => {
      const choice = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-first], [data-then]");
      if (!choice || sheet.busy) return;
      if (choice.dataset.first !== undefined) first = choice.dataset.first;
      else then = choice.dataset.then ?? "";
      draw();
    };
    sheet.body.onsubmit = (event) => {
      event.preventDefault();
      if (sheet.busy) return;
      submit(sheet, words, () => route([first, then].filter(Boolean)), done);
    };
    sheet.show();
  }
}

export class RelaySheet {
  private readonly sheet = new Sheet("device-sheet tree-sheet");

  constructor(private readonly words: TreeWords) {}

  /** Opens the sheet for `machine`; `set` makes it relay at the addresses
   *  given, or stop when none are. */
  open(machine: Node, set: (publicAt: string[], listen: string | null) => Promise<void>, done: () => void): void {
    const { sheet, words } = this;
    const relaysAt = machine.place?.relays_at ?? [];
    sheet.body.innerHTML = `
      ${sheet.head(words.relayTitle(machine.name), words.close)}
      <p class="confirm-what">${escape(words.relayWhat)}</p>
      <label class="field"><span class="field-label">${escape(words.publicAt)}</span>
        <input name="public" autocomplete="off" spellcheck="false" class="mono" placeholder="203.0.113.5:7444" value="${escape(relaysAt.join(", "))}">
        <span class="field-hint">${escape(words.publicHint)}</span></label>
      <label class="field"><span class="field-label">${escape(words.listenAt)}</span>
        <input name="listen" autocomplete="off" spellcheck="false" class="mono" placeholder="0.0.0.0:7444">
        <span class="field-hint">${escape(words.listenHint)}</span></label>
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot">${
        relaysAt.length ? `<button class="text-button quiet-button danger tree-stop" type="button">${escape(words.stopRelaying)}</button>` : "<span></span>"
      }<button class="sheet-go" type="submit">${escape(relaysAt.length ? words.relayChange : words.relayGo)}</button></footer>`;
    const field = (name: string) => sheet.body.querySelector<HTMLInputElement>(`[name=${name}]`)!.value;
    sheet.body.onclick = (event) => {
      if (!(event.target as HTMLElement).closest(".tree-stop") || sheet.busy) return;
      submit(sheet, words, () => set([], null), done);
    };
    sheet.body.onsubmit = (event) => {
      event.preventDefault();
      if (sheet.busy) return;
      const publicAt = field("public")
        .split(/[\s,，]+/)
        .filter(Boolean);
      const note = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
      if (!publicAt.length) {
        note.textContent = words.publicNeeded;
        note.hidden = false;
        return;
      }
      submit(sheet, words, () => set(publicAt, field("listen").trim() || null), done);
    };
    sheet.show();
  }
}

/** Does `act` with the sheet held; closes it when done, or says why not. */
function submit(sheet: Sheet, words: TreeWords, act: () => Promise<void>, done: () => void): void {
  const buttons = [...sheet.body.querySelectorAll<HTMLButtonElement>("button:not(.sheet-close)")];
  const note = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
  const go = sheet.body.querySelector<HTMLButtonElement>(".sheet-go")!;
  const label = go.textContent;
  sheet.busy = true;
  for (const button of buttons) button.disabled = true;
  go.textContent = words.working;
  note.hidden = true;
  act()
    .then(() => {
      sheet.busy = false;
      sheet.close();
      done();
    })
    .catch((error: unknown) => {
      sheet.busy = false;
      for (const button of buttons) button.disabled = false;
      go.textContent = label;
      note.textContent = words.failed(error instanceof Error ? error.message : String(error));
      note.hidden = false;
    });
}
