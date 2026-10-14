// A shared input kept in this page, standing in for the node's
// (`crates/fastctx-node/src/composer/`) in a specimen: edits by character
// identity, sending or queueing by whether the agent is busy, the queue's
// own operations, and files attached at once. One writer, no network; what
// it is for is seeing the input as a live World would show it.

import type { Attachment } from "../world/attachments";
import type { Character, ComposerDraft, ComposerMutation, ComposerPiece, ComposerQueued, ComposerSnapshot, ComposerTarget, ComposerUpdated } from "../world/composer";
import type { ComposerTransport } from "../world/composing";
import type { Address } from "../world/wire";

const scalars = (text: string) => [...text];

function empty(epoch: number): ComposerDraft {
  return { epoch, text: "", characters: [], attachments: [], uploads: [], files_on: null };
}

export function specimenComposer(who: Address, busy: () => boolean, queued: string[] = []): ComposerTransport {
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
    queue: queued.map((text, index) => ({ id: `q-${index}`, from: who.machine, draft: draftOf(text, 1), editing: false, sending: false, error: null })),
    unobserved: [],
    held: null,
    awaiting_turn: null,
  };
  const listeners = new Map<number, (piece: ComposerPiece) => void>();
  const changed = (next: Partial<ComposerSnapshot>, landed: ComposerUpdated["landed"] = null): ComposerUpdated => {
    snapshot = { ...snapshot, ...next, revision: snapshot.revision + 1 };
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
  return {
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
    update: (mutation) => {
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
          if (busy() || snapshot.awaiting_turn || snapshot.queue.some((one) => one.sending)) {
            serial += 1;
            const item: ComposerQueued = { id: `q-${serial}`, from: who.machine, draft: snapshot.draft, editing: false, sending: false, error: null };
            return Promise.resolve(changed({ draft: next, queue: [...snapshot.queue, item] }));
          }
          return Promise.resolve(changed({ draft: next }, { kind: "new_turn", turn: "specimen" }));
        }
        case "editing":
          return Promise.resolve(changed({ queue: snapshot.queue.map((item) => (item.id === mutation.id ? { ...item, editing: mutation.editing } : item)) }));
        case "delete":
          return Promise.resolve(changed({ queue: snapshot.queue.filter((item) => item.id !== mutation.id), unobserved: snapshot.unobserved.filter((item) => item.id !== mutation.id) }));
        case "move": {
          const item = snapshot.queue.find((one) => one.id === mutation.id);
          if (!item) return Promise.reject(new Error("that queued message no longer exists"));
          const rest = snapshot.queue.filter((one) => one.id !== mutation.id);
          const at = mutation.before ? rest.findIndex((one) => one.id === mutation.before) : rest.length;
          return Promise.resolve(changed({ queue: [...rest.slice(0, at), item, ...rest.slice(at)] }));
        }
        case "send_queued":
          return Promise.resolve(changed({ queue: snapshot.queue.filter((item) => item.id !== mutation.id) }, { kind: "joined_turn", turn: "specimen" }));
      }
    },
  };
}
