// A shared input kept in this page, standing in for the node's
// (`crates/fastctx-node/src/composer/`) in a specimen: edits by character
// identity, sending or queueing by whether the agent is busy, the queue's
// own operations, the places of commands waiting among the words, and files
// attached at once. One writer, no network; what it is for is seeing the
// input as a live World would show it.

import type { Attachment } from "../world/attachments";
import type { Character, ComposerDraft, ComposerMutation, ComposerPiece, ComposerQueued, ComposerSnapshot, ComposerTarget, ComposerUpdated } from "../world/composer";
import type { ComposerTransport } from "../world/composing";
import type { Address } from "../world/wire";

const scalars = (text: string) => [...text];

function empty(epoch: number): ComposerDraft {
  return { epoch, text: "", characters: [], attachments: [], uploads: [], files_on: null };
}

/** The input, and the places it keeps for commands waiting among its words. */
export type SpecimenComposer = ComposerTransport & { place(id: string): void; forget(id: string): void };

/** How long a node far away and busy takes: to have what it is asked
 *  (`taken`), and then to hand words sent to the agent (`heard`). */
export interface SpecimenSlowness {
  taken: number;
  heard: number;
}

/** `retrying`, where given, is told the id of words sent while the agent is
 *  idle, which then wait for their host to start as a node keeps them. */
export function specimenComposer(who: Address, busy: () => boolean, queued: string[] = [], retrying?: (delivery: string) => void, slow?: SpecimenSlowness): SpecimenComposer {
  let serial = 0;
  const draftOf = (text: string, epoch: number): ComposerDraft => {
    serial += 1;
    const operation = `specimen-${serial}`;
    const characters = scalars(text).map((one, index) => ({ id: { operation, index }, text: one }));
    return { ...empty(epoch), text, characters };
  };
  let snapshot: ComposerSnapshot = {
    revision: 1,
    draft: empty(1),
    // Ids as the node makes them, with the time each was queued.
    queue: queued.map((text, index) => ({ id: `queued-${Date.now() - (queued.length - index) * 60_000}-${index}-0`, from: who.machine, draft: draftOf(text, 1), editing: false, sending: false, error: null })),
    unobserved: [],
    held: null,
    awaiting_turn: null,
  };
  // The one order of words and commands' places, as the node keeps it.
  let order = snapshot.queue.map((one) => one.id);
  const commands = new Set<string>();
  const arranged = (queue: readonly ComposerQueued[]): Pick<ComposerSnapshot, "queue" | "commands"> => {
    const words = new Map(queue.map((one) => [one.id, one]));
    order = order.filter((id) => commands.has(id) || words.has(id));
    for (const one of queue) if (!order.includes(one.id)) order.push(one.id);
    let after: string | null = null;
    const places: { id: string; after: string | null }[] = [];
    for (const id of order) {
      if (commands.has(id)) places.push({ id, after });
      else after = id;
    }
    return { queue: order.flatMap((id) => words.get(id) ?? []), commands: places };
  };
  const listeners = new Map<number, (piece: ComposerPiece) => void>();
  // Words sent straight on that the agent has not heard yet: a node does
  // not list them, and words sent meanwhile wait behind them.
  let straight = 0;
  const changed = (next: Partial<ComposerSnapshot>, landed: ComposerUpdated["landed"] = null): ComposerUpdated => {
    const merged = { ...snapshot, ...next };
    snapshot = { ...merged, ...arranged(merged.queue), revision: snapshot.revision + 1 };
    for (const listener of listeners.values()) listener({ kind: "snapshot", watch: "specimen", composer: snapshot });
    return { composer: snapshot, landed };
  };
  const withDraft = (target: ComposerTarget, change: (draft: ComposerDraft) => ComposerDraft): Partial<ComposerSnapshot> => {
    if (target.kind === "draft") return { draft: change(snapshot.draft) };
    return { queue: snapshot.queue.map((item) => (item.id === target.id ? { ...item, draft: change(item.draft) } : item)) };
  };
  const edited = (draft: ComposerDraft, mutation: Extract<ComposerMutation, { do: "edit" }>): ComposerDraft => {
    const gone = new Set(mutation.delete.map((id) => `${id.operation}:${id.index}`));
    const kept = draft.characters.filter((one) => !gone.has(`${one.id.operation}:${one.id.index}`));
    const at = mutation.after ? kept.findIndex((one) => one.id.operation === mutation.after!.operation && one.id.index === mutation.after!.index) + 1 : 0;
    const added: Character[] = scalars(mutation.insert).map((one, index) => ({ id: { operation: mutation.operation, index }, text: one }));
    const characters = [...kept.slice(0, at), ...added, ...kept.slice(at)];
    return { ...draft, characters, text: characters.map((one) => one.text).join("") };
  };
  const attachment = (target: ComposerTarget, name: string, type: string, size: number): { attachment: Attachment; updated: { Ok: ComposerUpdated } } => {
    serial += 1;
    const one: Attachment = { id: `a-${serial}`, name, media_type: type || "application/octet-stream", size, sha256: "", owner: who, town: null, path: `C:/Users/me/.fastctx/attachments/${name}` };
    return { attachment: one, updated: { Ok: changed(withDraft(target, (draft) => ({ ...draft, attachments: [...draft.attachments, one] }))) } };
  };

  function later<T>(ms: number, then: () => Promise<T>): Promise<T> {
    return new Promise((resolve) => setTimeout(resolve, ms)).then(then);
  }

  function apply(mutation: ComposerMutation): Promise<ComposerUpdated> {
    switch (mutation.do) {
      case "edit":
        return Promise.resolve(changed(withDraft(mutation.target, (draft) => edited(draft, mutation))));
      case "attach":
        return Promise.resolve(changed(withDraft(mutation.target, (draft) => ({ ...draft, attachments: [...draft.attachments, mutation.attachment] }))));
      case "detach":
        return Promise.resolve(changed(withDraft(mutation.target, (draft) => ({ ...draft, attachments: draft.attachments.filter((one) => one.id !== mutation.attachment) }))));
      case "files_on":
        return Promise.resolve(changed(withDraft(mutation.target, (draft) => ({ ...draft, files_on: mutation.files_on }))));
      case "send": {
        if (mutation.epoch !== snapshot.draft.epoch) return Promise.reject(new Error("this draft was already sent; keep your local edit and apply it to the new epoch"));
        const next = empty(snapshot.draft.epoch + 1);
        // As the node does: words wait only while the agent works or
        // others are on their way.
        if (busy() || snapshot.awaiting_turn || snapshot.queue.some((one) => one.sending) || straight) {
          serial += 1;
          const item: ComposerQueued = { id: `queued-${Date.now()}-${serial}-0`, from: who.machine, draft: snapshot.draft, editing: false, sending: false, error: null };
          return Promise.resolve(changed({ draft: next, queue: [...snapshot.queue, item] }));
        }
        if (slow) {
          // As a node does while its agent's host is slow to take them:
          // the words are its own at once, on their way until it does.
          straight += 1;
          changed({ draft: next });
          return later(slow.heard, () => {
            straight -= 1;
            return Promise.resolve(changed({}, { kind: "new_turn", turn: "specimen" }));
          });
        }
        if (retrying) {
          serial += 1;
          const delivery = `words-${Date.now()}-${serial}`;
          retrying(delivery);
          return Promise.resolve({ ...changed({ draft: next }, { kind: "waiting", why: null, answered: true }), delivery });
        }
        return Promise.resolve(changed({ draft: next }, { kind: "new_turn", turn: "specimen" }));
      }
      case "editing":
        return Promise.resolve(changed({ queue: snapshot.queue.map((item) => (item.id === mutation.id ? { ...item, editing: mutation.editing } : item)) }));
      case "delete":
        return Promise.resolve(changed({ queue: snapshot.queue.filter((item) => item.id !== mutation.id), unobserved: snapshot.unobserved.filter((item) => item.id !== mutation.id) }));
      case "move": {
        if (!order.includes(mutation.id)) return Promise.reject(new Error("that queued message no longer exists"));
        const rest = order.filter((one) => one !== mutation.id);
        const found = mutation.before ? rest.indexOf(mutation.before) : -1;
        const at = found < 0 ? rest.length : found;
        order = [...rest.slice(0, at), mutation.id, ...rest.slice(at)];
        return Promise.resolve(changed({}));
      }
      case "send_queued":
        if (straight || snapshot.queue.some((item) => item.sending)) return Promise.reject(new Error("another composer message is being delivered"));
        return Promise.resolve(changed({ queue: snapshot.queue.filter((item) => item.id !== mutation.id) }, { kind: "joined_turn", turn: "specimen" }));
    }
  }

  return {
    place: (id) => {
      commands.add(id);
      order.push(id);
      changed({});
    },
    forget: (id) => {
      commands.delete(id);
      order = order.filter((one) => one !== id);
      changed({});
    },
    read: () => Promise.resolve({ composer: snapshot, landed: null }),
    unwatch: (id) => {
      listeners.delete(id);
      return Promise.resolve();
    },
    watch: (onPiece) => {
      serial += 1;
      listeners.set(serial, onPiece);
      return Promise.resolve({ id: serial, watch: "specimen", composer: snapshot });
    },
    attachFile: (scope, file) => Promise.resolve(attachment(scope.target, file.name, file.type, file.size)),
    attachPath: (scope, path) => Promise.resolve(attachment(scope.target, path.split(/[\\/]/).pop() ?? path, "", 0)),
    update: (mutation) => (slow ? later(slow.taken, () => apply(mutation)) : apply(mutation)),
  };
}
