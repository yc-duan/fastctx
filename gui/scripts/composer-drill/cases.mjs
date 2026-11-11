import { Clock } from "./clock.mjs";
import { journalCases } from "./journal-cases.mjs";
import { inputCases } from "./input-cases.mjs";

const assert = (yes, why) => { if (!yes) throw new Error(why); };
const blank = () => ({ revision: 1, draft: { epoch: 0, text: "", characters: [], attachments: [], uploads: [], files_on: null }, queue: [], unobserved: [], commands: [], held: null, awaiting_turn: null });
const metadata = () => ({ revision: 1, draft: { id: "draft", revision: 0, epoch: 0, attachments: [], uploads: [], files_on: null }, queue: [], unobserved: [], commands: [], held: null, awaiting_turn: null });
const initial = (state = metadata()) => ({ kind: "state", version: 1, mutations: true, state, documents: [], vectors: [] });

/** Focused GUI counterexamples. The multi-device run requires the real crate driver. */
export async function run(moduleURL, selected, seed = 1, legacyURL) {
  const clock = new Clock(); clock.install(seed);
  const product = await import(moduleURL);
  const { NativeComposer, ComposerDocument, ComposerSession, bindComposer, encode, Y } = product;
  const nativeFixture = (text = "hello", mutations = true) => {
    const source = new Y.Doc(); source.getText("text").insert(0, text);
    let callback; let revision = 1; let epoch = 0;
    const inputs = []; const legacy = [];
    const state = () => ({ ...metadata(), revision, draft: { ...metadata().draft, epoch, revision } });
    const frame = (update = Y.encodeStateAsUpdate(source)) => ({ kind: "state", version: 1, mutations, state: state(), documents: [{ document: "draft", revision, update: encode(update) }] });
    const snapshot = () => {
      const text = source.getText("text"); const characters = []; let offset = 0;
      for (const scalar of text.toString()) {
        const id = Y.createRelativePositionFromTypeIndex(text, offset).item;
        characters.push({ id: { operation: `yjs:${id.client}:${id.clock}`, index: 0 }, text: scalar }); offset += scalar.length;
      }
      return { ...blank(), revision, draft: { ...blank().draft, epoch, text: text.toString(), characters } };
    };
    const publish = (value) => callback?.({ kind: "native", watch: "watch", frame: value });
    const transport = {
      read: async () => ({ composer: snapshot(), landed: null }),
      watchDocuments: async (_, receive) => { callback = receive; return { id: 1, watch: "watch", frame: frame() }; },
      documentInput: async (_, input) => {
        inputs.push(structuredClone(input));
        if (input.kind === "update") {
          Y.applyUpdate(source, Uint8Array.from(atob(input.update), (character) => character.charCodeAt(0))); revision++;
          publish({ kind: "update", revision, document: { document: "draft", revision, update: input.update } });
          publish({ kind: "saved", number: input.number, document: "draft", revision, document_revision: revision });
        } else if (input.mutation.do === "withdraw") publish({ kind: "refused", number: input.number, failure: { message: "these words were already sent" } });
        else if (!mutations) publish({ kind: "refused", number: input.number, failure: { message: "unsupported native mutation" } });
      },
      update: async (mutation) => {
        legacy.push(structuredClone(mutation));
        let update; const changed = (value) => { update = value; }; source.on("update", changed);
        if (mutation.do === "edit") {
          const characters = snapshot().draft.characters; const key = (id) => `${id.operation}/${id.index}`;
          const deleted = new Set(mutation.delete.map(key)); let offset = 0; let at = 0; const ranges = [];
          for (const character of characters) {
            if (deleted.has(key(character.id))) ranges.push({ at: offset, length: character.text.length });
            offset += character.text.length;
            if (mutation.after && key(character.id) === key(mutation.after)) at = offset;
          }
          const before = ranges.reduce((count, range) => count + (range.at < at ? range.length : 0), 0);
          source.transact(() => { for (const range of ranges.reverse()) source.getText("text").delete(range.at, range.length); source.getText("text").insert(at - before, mutation.insert); });
          revision++; publish(frame(update));
        }
        if (mutation.do === "send") { source.getText("text").delete(0, source.getText("text").length); epoch++; revision++; publish(frame(update)); }
        source.off("update", changed);
        return { composer: snapshot(), landed: { kind: "new_turn", turn: "turn" } };
      },
      watch: async () => ({ id: 2, watch: "legacy", composer: snapshot() }),
      unwatch: async () => {},
    };
    return { source, transport, inputs, legacy, frame, publish, snapshot };
  };
  const memoryJournal = (entries = []) => {
    const data = new Map(entries);
    return { read: async () => new Map(data), write: async (key, value) => { if (value) data.set(key, structuredClone(value)); else data.delete(key); } };
  };
  const cases = {
    ...journalCases(product, legacyURL, blank, initial, assert),
    ...inputCases(product, blank, initial, assert, clock, nativeFixture, legacyURL),
    async A4prepared() {
      const fixture = nativeFixture(); const session = await ComposerSession.open({ resident: "composer-prepared", machine: null }, { transport: fixture.transport, journal: memoryJournal(), recoveryKey: JSON.stringify([null, "composer-prepared", "prepared"]) });
      const first = session.send(); void first.sent.catch(() => {}); await clock.flush();
      void session.setFilesOn("elsewhere").catch(() => {});
      session.draft.edit(0, 0, "next");
      const second = session.send(); let finished = false; void second.sent.then(() => { finished = true; }, () => { finished = true; });
      const sent = fixture.inputs.find((input) => input.mutation?.do === "send_document");
      Y.applyUpdate(fixture.source, Uint8Array.from(atob(sent.mutation.take), (one) => one.charCodeAt(0)));
      const state = fixture.frame(); state.state.revision = 2; state.state.draft.epoch = 1; state.state.draft.revision = 2;
      fixture.publish(state);
      fixture.publish({ kind: "result", revision: 2, operation: { operation: sent.mutation.operation, accepted: false, landed: { kind: "new_turn", turn: "turn" }, delivery: "delivery", failure: null } });
      await clock.flush();
      try {
        assert(fixture.inputs.some((input) => input.mutation?.do === "files_on"), "preparation did not reach its unanswered metadata request");
        await second.withdraw(); await clock.flush();
        assert(finished && session.draft.view.characters.map((one) => one.text).join("") === "next", "withdrawal did not interrupt unanswered pre-send metadata");
      } finally { await session.close(); fixture.source.destroy(); }
      return { preparationInterrupted: true };
    },
    async A4restoredSeal() {
      const fixture = nativeFixture(); const journal = memoryJournal();
      const options = { transport: fixture.transport, journal, recoveryKey: JSON.stringify([null, "composer-probe", "restored-seal"]) };
      const session = await ComposerSession.open({ resident: "composer-probe", machine: null }, options);
      const sending = session.send(); void sending.sent.catch(() => {}); await clock.flush();
      await session.close();
      const reopened = await ComposerSession.open({ resident: "composer-probe", machine: null }, options);
      try {
        assert(reopened.draft.view.characters.length === 0, "unresolved transmitted send returned its words on reopen without node withdrawal");
        assert(reopened.state.onTheirWay === 1, "reopen did not restore the captured send's seal");
      } finally { await reopened.close(); fixture.source.destroy(); }
      return { restoredSealed: true };
    },
    async A4reopen() {
      const native = new NativeComposer([]); native.receive(initial());
      const inputs = []; native.transmit = async (input) => inputs.push(structuredClone(input)); native.connect();
      void native.mutate({ operation: "same-send", do: "send", epoch: 0 }).catch(() => {}); await clock.flush();
      const saved = { snapshot: blank(), target: { kind: "draft" }, pending: [], contexts: [], history: [], retired: [], native: native.journal("draft") }; native.dispose();
      const reopened = new NativeComposer([saved]); reopened.receive(initial());
      let refused = false;
      try { await reopened.withdraw("same-send"); } catch { refused = true; }
      assert(refused && reopened.pending, "reopened transmitted send was cancelled locally while disconnected");
      reopened.dispose(); return { transmittedRetained: true };
    },
    async A4success() {
      const native = new NativeComposer([]); native.receive(initial());
      const sent = []; native.transmit = async (input) => {
        sent.push(structuredClone(input));
        if (input.mutation?.do === "withdraw") native.receive({ kind: "result", revision: 1, operation: { operation: input.mutation.operation, accepted: false, landed: null, delivery: null, failure: null } });
      };
      native.connect();
      void native.mutate({ operation: "withdrawn-send", do: "send", epoch: 0 }).catch(() => {}); await clock.flush();
      assert(await native.withdraw("withdrawn-send") === "withdrawn", "confirmed withdrawal did not complete");
      const count = sent.filter((input) => input.mutation?.operation === "withdrawn-send").length;
      await clock.advance(90_000);
      assert(sent.filter((input) => input.mutation?.operation === "withdrawn-send").length === count && !native.pending, "confirmed withdrawal replayed the original send");
      native.dispose(); return { originalTransmissions: count, replayedAfterWithdrawal: 0 };
    },
    async C2() {
      const native = new NativeComposer([]); native.receive(initial()); const inputs = [];
      native.transmit = async (input) => inputs.push(structuredClone(input)); native.connect();
      void native.mutate({ operation: "same-operation", do: "send", epoch: 0 }).catch(() => {}); await clock.flush();
      for (let attempt = 0; attempt < 20; attempt++) await clock.advance(30_000);
      assert(inputs.length >= 21 && inputs.every((input) => JSON.stringify(input) === JSON.stringify(inputs[0])), "retry changed a mutation's number or content");
      native.receive({ kind: "result", revision: 1, operation: { operation: "same-operation", accepted: false, landed: { kind: "new_turn", turn: "turn" }, delivery: "delivery", failure: null } });
      const count = inputs.length; await clock.advance(90_000);
      assert(inputs.length === count, "a settled mutation kept replaying");
      native.dispose(); return { identicalTransmissions: count, backendOnce: "requires real crate driver" };
    },
    async A4() {
      const fixture = nativeFixture();
      const session = await ComposerSession.open({ resident: "composer-probe", machine: null }, { transport: fixture.transport, journal: memoryJournal(), recoveryKey: JSON.stringify([null, "composer-probe", "withdraw"]) });
      const sending = session.send(); void sending.sent.catch(() => {}); await clock.flush();
      try {
        assert(fixture.inputs.some((input) => input.mutation?.do === "send_document"), "send was not transmitted by fixture");
        await Promise.resolve(sending.withdraw()); await clock.flush();
        assert(fixture.inputs.some((input) => input.mutation?.do === "withdraw"), "transmitted send was withdrawn without asking the node");
        assert(session.draft.view.characters.length === 0, "already-sent withdrawal put its words back into the input");
      } finally { await session.close(); fixture.source.destroy(); }
      return { asked: true, originalStayedSealed: true };
    },
    async A2() {
      const fixture = nativeFixture("hello", false);
      const session = await ComposerSession.open({ resident: "composer-probe", machine: null }, { transport: fixture.transport, journal: memoryJournal(), recoveryKey: JSON.stringify([null, "composer-probe", "old-send"]) });
      const sending = session.send(); void sending.sent.catch(() => {}); await clock.flush();
      try {
        assert(!fixture.inputs.some((input) => input.kind === "mutate"), "mutations=false received a new native mutation");
        assert(fixture.legacy.some((mutation) => mutation.do === "send" && !("take" in mutation)), "old node did not receive a legacy send");
      } finally { await session.close(); fixture.source.destroy(); }
      return { legacySend: true };
    },
    async A6() {
      const original = new NativeComposer([]); original.receive(initial()); original.replica("draft").edit(0, 0, "offline");
      const saved = { snapshot: original.snapshot(false), target: { kind: "draft" }, pending: [], contexts: [], history: [], retired: [], native: original.journal("draft") }; original.dispose();
      const fixture = nativeFixture(""); let synchronized = 0;
      fixture.transport.watchDocuments = async () => null;
      fixture.transport.syncText = async (input) => { synchronized++; Y.applyUpdate(fixture.source, Uint8Array.from(atob(input.update), (character) => character.charCodeAt(0))); return { composer: fixture.snapshot(), landed: null }; };
      const session = await ComposerSession.open({ resident: "composer-probe", machine: null }, { transport: fixture.transport, journal: memoryJournal([["native:replica:draft", saved]]), recoveryKey: JSON.stringify([null, "composer-probe", "old-open"]) });
      try { assert(synchronized > 0 && session.draft.view.characters.map((character) => character.text).join("") === "offline", "initial old-node fallback hid the native outbox"); }
      finally { await session.close(); fixture.source.destroy(); }
      return { synchronized, text: "offline" };
    },
    async A7() {
      let saved = [];
      for (let step = 0; step < 300; step++) {
        const native = new NativeComposer(saved); native.receive(initial());
        native.assertEditable("draft");
        const replica = native.replica("draft"); replica.edit(replica.characters.length, replica.characters.length, "x");
        saved = [{ snapshot: blank(), target: { kind: "draft" }, pending: [], contexts: [], history: [], retired: [], native: native.journal("draft") }];
        native.dispose();
      }
      const native = new NativeComposer(saved); native.receive(initial());
      assert(native.replica("draft").content === "x".repeat(300), "offline reopen lost text");
      native.assertEditable("draft"); native.dispose();
      return { reopens: 300, scalars: 300 };
    },
    async A8() {
      const composer = new ComposerDocument({ snapshot: blank(), target: { kind: "draft" }, send: () => new Promise(() => {}) });
      const field = document.createElement("textarea"); document.body.append(field);
      const stop = bindComposer(field, composer);
      const original = composer.edit.bind(composer);
      composer.edit = () => { throw new Error("injected editor failure"); };
      const prevent = (event) => event.preventDefault(); window.addEventListener("error", prevent);
      try {
        field.dispatchEvent(new InputEvent("beforeinput", { inputType: "insertText", data: "a", cancelable: true }));
        field.value = "a"; field.setSelectionRange(1, 1);
        field.dispatchEvent(new InputEvent("input", { inputType: "insertText", data: "a" }));
        assert(!composer.view.state.composing, "ordinary input throw left composing=true");
        composer.edit = original;
        field.dispatchEvent(new CompositionEvent("compositionstart"));
        field.value = "中"; field.setSelectionRange(1, 1);
        composer.edit = () => { throw new Error("injected composition failure"); };
        field.dispatchEvent(new CompositionEvent("compositionend", { data: "中" }));
        await clock.advance(0);
        assert(!composer.view.state.composing, "composition throw left composing=true");
      } finally { composer.edit = original; stop(); composer.dispose(); field.remove(); window.removeEventListener("error", prevent); }
      return { ordinary: true, composition: true };
    },
    async A12() {
      const state = metadata(); state.queue.push({ id: "queued", from: "machine", document: { ...state.draft, id: "queue-doc" }, editing: false, sending: false, error: null });
      const native = new NativeComposer([]); native.receive(initial(state));
      native.persist = async (id) => { if (id === "draft") throw new Error("draft checkpoint failed"); };
      const source = new Y.Doc(); let update;
      source.on("update", (value) => { update = value; }); source.getText("text").insert(0, "a");
      native.receive({ kind: "update", revision: 2, document: { document: "draft", revision: 1, update: encode(update) } });
      await clock.advance(1000);
      assert(native.status("draft").error, "checkpoint failure not visible");
      native.replica("queue-doc").edit(0, 0, "b"); native.transmit = async () => {}; native.connect(); await clock.flush();
      assert(native.status("draft").error, "unrelated successful queue write erased draft checkpoint failure");
      native.dispose(); source.destroy(); return { scoped: true };
    },
    async C1reopen() {
      const fixture = nativeFixture(""); const data = memoryJournal(); let rejectWrite;
      const originalWrite = data.write;
      let blocked = false;
      data.write = (key, value) => blocked ? new Promise((_, reject) => { rejectWrite = reject; }) : originalWrite(key, value);
      const who = { resident: "composer-reopen", machine: null };
      const session = await ComposerSession.open(who, { transport: fixture.transport, journal: data });
      await clock.flush();
      // Persistence stalls before the exact update can reach transport.
      blocked = true; session.draft.edit(0, 0, "kept on failed close"); await clock.flush();
      const closing = session.close(); void closing.catch(() => {}); await clock.flush();
      const opening = ComposerSession.open(who, { transport: fixture.transport, journal: data });
      await clock.flush();
      assert(rejectWrite, "close did not reach the injected journal stall");
      blocked = false; rejectWrite(new Error("injected failed handoff"));
      await closing.catch(() => {});
      const reopened = await opening;
      try {
        assert(reopened.recoveryKey === session.recoveryKey, "quick reopen abandoned the failed writer's recovery key");
        assert(reopened.draft.view.characters.map((one) => one.text).join("") === "kept on failed close", "quick reopen did not recover its failed writer's text");
      } finally { await reopened.close(); fixture.source.destroy(); }
      return { sameWriter: true, textRecovered: true };
    },
    async C1() {
      let fail = false; let watches = 0;
      const journal = { read: async () => new Map(), write: async () => { if (fail) throw new Error("close persistence failed"); } };
      const transport = { read: async () => ({ composer: blank(), landed: null }), update: async () => { throw new Error("unexpected mutation"); }, watch: async () => ({ id: ++watches, watch: "watch", composer: blank() }), unwatch: async () => {} };
      const key = JSON.stringify([null, "composer-probe", "closing"]);
      const session = await ComposerSession.open({ resident: "composer-probe", machine: null }, { transport, journal, recoveryKey: key });
      await clock.flush(); fail = true;
      await session.close().catch(() => {});
      const locks = await navigator.locks.query();
      assert(!locks.held.some((lock) => lock.name === `world-composer:${key}`), "failed close retained writer lock");
      let closed = false; try { await session.reconnect(); } catch { closed = true; }
      assert(closed, "failed close left session reusable"); return { closed: true };
    },
  };
  const results = [];
  for (const name of selected ?? Object.keys(cases)) {
    try { results.push({ name, passed: true, ...await cases[name]() }); }
    catch (error) { results.push({ name, passed: false, error: String(error?.stack ?? error).replace(/data:text\/javascript;base64,[A-Za-z0-9+/=]+/g, "<bundle>") }); }
  }
  return { seed, results, passed: results.every((result) => result.passed) };
}
