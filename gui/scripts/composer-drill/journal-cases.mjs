/** Both versions use the same real IndexedDB origin, as an application downgrade does. */
export function journalCases(product, legacyURL, blank, initial, assert) {
  const { NativeComposer, composerJournalStore } = product;
  const savedDraft = (native) => ({ snapshot: blank(), target: { kind: "draft" }, pending: [], contexts: [], history: [], retired: [], native: native.journal("draft") });
  return {
    async A5isolation() {
      const key = JSON.stringify([null, "composer-probe", "downgrade"]);
      const native = new NativeComposer([]); native.receive(initial()); native.replica("draft").edit(0, 0, "retained原文😀");
      const saved = savedDraft(native); native.dispose();
      const current = await composerJournalStore(key); await current.write("native:replica:draft", saved); current.close?.();
      const oldVersion = await import(legacyURL);
      const old = await oldVersion.composerJournalStore(key);
      try {
        const visible = await old.read();
        assert(![...visible.values()].some((journal) => journal.native), "released GUI can open the native replica as a draft");
        // The old fallback can write the same key. It must have no access to the
        // new application's checkpoint or exact pending updates.
        await old.write("native:replica:draft", { ...saved, native: undefined, pending: [] });
        const reopened = await composerJournalStore(key);
        try {
          const journals = await reopened.read();
          const restored = new NativeComposer([...journals.values()]); restored.receive(initial());
          try { assert(restored.replica("draft").content === "retained原文😀", "old format write erased the pending native text"); }
          finally { restored.dispose(); }
        } finally { reopened.close?.(); }
      } finally { old.close?.(); }
      return { nativeHidden: true, oldWriteRetained: true };
    },
    async A5mirror() {
      const key = JSON.stringify([null, "composer-probe", "plaintext-mirror"]);
      const native = new NativeComposer([]); native.receive(initial()); native.replica("draft").edit(0, 0, "offline plaintext");
      const current = await composerJournalStore(key); await current.write("native:replica:draft", savedDraft(native)); current.close?.(); native.dispose();
      const oldVersion = await import(legacyURL);
      const transport = { read: async () => ({ composer: blank(), landed: null }), update: async () => { throw new Error("no mirror edit was expected"); }, watch: async () => ({ id: 1, watch: "old", composer: blank() }), unwatch: async () => {} };
      const session = await oldVersion.ComposerSession.open({ resident: "composer-probe", machine: null }, { transport, recoveryKey: key });
      try { assert(session.draft.view.characters.map((one) => one.text).join("") === "offline plaintext", "legacy plaintext mirror is not connected; exact native data remains in its isolated database"); }
      finally { await session.close(); }
      return { plaintextVisibleToReleasedGUI: true };
    },
    async A5import() {
      const key = JSON.stringify([null, "composer-probe", "development-import"]);
      const native = new NativeComposer([]); native.receive(initial()); native.replica("draft").edit(0, 0, "old native tail");
      const saved = savedDraft(native); native.dispose();
      // Exercise the original journal form as well: it had outbox but no rows.
      delete saved.native.updates;
      const oldVersion = await import(legacyURL); const old = await oldVersion.composerJournalStore(key);
      await old.write("native:replica:draft", saved);
      const current = await composerJournalStore(key);
      await current.read(); current.close?.();
      assert((await old.read()).size === 0, "import left native heads exposed in the legacy database");
      const reopened = await composerJournalStore(key);
      try {
        const journals = await reopened.read();
        const restored = new NativeComposer([...journals.values()]); restored.receive(initial());
        try {
          assert(restored.replica("draft").content === "old native tail", "reopened import lost pending text");
          assert(JSON.stringify(restored.journal("draft").outbox) === JSON.stringify(saved.native.outbox), "reopened import lost the exact pending inputs");
        } finally { restored.dispose(); }
      } finally { reopened.close?.(); old.close?.(); }
      return { imported: true, reopened: true, exactOutbox: true };
    },
  };
}
