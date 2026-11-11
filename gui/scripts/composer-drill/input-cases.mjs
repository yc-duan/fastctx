export function inputCases(product, blank, initial, assert, clock, nativeFixture, legacyURL) {
  const { NativeComposer, ComposerSession, bindComposer, encode, Y } = product;
  return {
    async A7budget() {
      const source = new Y.Doc(); const text = source.getText("text"); const changes = [];
      source.on("update", (update) => changes.push(update));
      for (let round = 0; round < 24; round++) { text.insert(0, "x".repeat(30_000)); text.delete(0, 30_000); }
      const update = encode(Y.mergeUpdates(changes)); source.destroy();
      const outbox = Array.from({ length: 10 }, (_, number) => ({ kind: "update", number: number + 1, document: "draft", update }));
      const empty = new Y.Doc(); const checkpoint = encode(Y.encodeStateAsUpdate(empty)); empty.destroy();
      const native = new NativeComposer([{ snapshot: blank(), target: { kind: "draft" }, pending: [], contexts: [], history: [], retired: [], native: { document: "draft", checkpoint, next: 10, outbox, sent: outbox.map((input) => input.number) } }]);
      native.receive(initial());
      let blocked = false; try { native.assertEditable("draft"); } catch { blocked = true; }
      assert(blocked && native.status("draft").error, "full byte budget was not visible");
      let saved = 0; native.transmit = async (input) => { saved++; native.receive({ kind: "saved", document: "draft", number: input.number, revision: 1, document_revision: 1 }); };
      native.connect(); for (let turn = 0; turn < 100 && native.pending; turn++) await clock.flush();
      native.assertEditable("draft");
      assert(!native.status("draft").error && !native.pending && saved === 10, "Saved did not automatically heal the byte budget");
      native.dispose(); return { encodedBytes: update.length * 10, saved, recovered: true };
    },
    async A7sealed() {
      const native = new NativeComposer([]); native.receive(initial()); const sent = [];
      native.replica("draft").edit(0, 0, "a"); const hold = native.hold("draft", 0);
      native.replica("draft").edit(1, 1, "b");
      assert(native.journal("draft").outbox.length === 2, "unsent merge crossed the captured prefix");
      native.transmit = async (input) => sent.push(structuredClone(input)); native.connect(); await clock.flush();
      const first = JSON.stringify(sent[0]); native.replica("draft").edit(2, 2, "c"); await clock.flush();
      assert(sent.length === 1 && JSON.stringify(native.journal("draft").outbox[0]) === first, "transmitted update was changed by later typing");
      native.receive({ kind: "saved", document: "draft", number: sent[0].number, revision: 1, document_revision: 1 }); await clock.flush();
      assert(sent.length === 1, "post-seal update went before the captured send");
      native.release("draft", hold); await clock.flush();
      assert(sent.length === 2, "unsealing did not release the merged suffix");
      native.dispose(); return { capturedAndSuffix: 2, transmittedImmutable: true };
    },
    async B4() {
      const fixture = nativeFixture("😀".repeat(30_000));
      const session = await ComposerSession.open({ resident: "composer-cap", machine: null }, { transport: fixture.transport, recoveryKey: JSON.stringify([null, "composer-probe", "cap"]) });
      const field = document.createElement("textarea"); const notice = document.createElement("p"); document.body.append(field, notice);
      const stop = bindComposer(field, session.draft, () => {}, notice);
      try {
        assert(!notice.hidden && notice.textContent.includes("30,000"), "at-limit notice did not name the scalar limit");
        let refused = false; try { session.draft.edit(30_000, 30_000, "x"); } catch (error) { refused = error.message === "the text is longer than this input holds"; }
        assert(refused && session.draft.view.characters.length === 30_000, "non-BMP draft exceeded its scalar cap");
        session.draft.edit(29_999, 30_000, "");
        assert(session.draft.view.characters.length === 29_999, "a full input could not shrink");
      } finally { stop(); field.remove(); notice.remove(); await session.close(); fixture.source.destroy(); }
      return { scalarCap: 30_000, nonBMP: true, deletionAllowed: true };
    },
    async LegacyTiming() { return this.B4timing(await import(legacyURL)); },
    async B4timing(version = product) {
      const { ComposerSession, bindComposer } = version;
      const results = [];
      for (const scalar of ["x", "😀"]) {
        const fixture = nativeFixture(scalar.repeat(29_999));
        const session = await ComposerSession.open({ resident: "composer-timing", machine: null }, { transport: fixture.transport, recoveryKey: JSON.stringify([null, "composer-probe", `timing-${scalar}`]) });
        const field = document.createElement("textarea"); document.body.append(field); const stop = bindComposer(field, session.draft);
        const timings = []; const endToEnd = [];
        const saved = () => !session.draft.view.state.pending ? Promise.resolve() : new Promise((resolve) => {
          let unsubscribe = () => {}; unsubscribe = session.draft.subscribe((view) => { if (!view.state.pending) { unsubscribe(); resolve(); } });
        });
        try {
          for (let step = 0; step < 1_020; step++) {
            const adding = step % 2 === 0; const before = performance.now();
            const prior = field.value; field.setSelectionRange(prior.length, prior.length);
            field.dispatchEvent(new InputEvent("beforeinput", { inputType: adding ? "insertText" : "deleteContentBackward", data: adding ? scalar : null, cancelable: true }));
            field.value = adding ? prior + scalar : prior.slice(0, -scalar.length); field.setSelectionRange(field.value.length, field.value.length);
            field.dispatchEvent(new InputEvent("input", { inputType: adding ? "insertText" : "deleteContentBackward", data: adding ? scalar : null }));
            const elapsed = performance.now() - before;
            await saved();
            if (step >= 20) { timings.push(elapsed); endToEnd.push(performance.now() - before); }
          }
          const percentile = (values, at) => [...values].sort((a, b) => a - b)[Math.ceil(values.length * at) - 1];
          results.push({ scalar, edits: timings.length, mainThreadMs: { p50: percentile(timings, .5), p99: percentile(timings, .99) }, throughSavedMs: { p50: percentile(endToEnd, .5), p99: percentile(endToEnd, .99) }, inputMaxBytes: Math.max(...fixture.inputs.map((input) => new TextEncoder().encode(JSON.stringify(input)).length)) });
        } finally { stop(); field.remove(); await session.close(); fixture.source.destroy(); }
      }
      return { transport: "GUI protocol stub; real IndexedDB and textarea", results };
    },
  };
}
