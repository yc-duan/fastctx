/** These cases exercise actual GUI send/seal/outbox logic. Host acceptance is a
 * protocol stub; the real node's multi-device driver is a separate drill. */
export function recoveryCases(product, assert, clock, fixture, memoryJournal) {
  const { ComposerSession } = product;
  const prepare = async (name, after = "", body = "hello") => {
    const node = fixture(body); const journal = memoryJournal();
    const who = { resident: "composer-recovery", machine: null };
    const options = { transport: node.transport, journal, recoveryKey: JSON.stringify([null, who.resident, name]) };
    const original = await ComposerSession.open(who, options);
    const sending = original.send(); void sending.sent.catch(() => {}); await clock.flush();
    const input = node.inputs.find((one) => one.mutation?.do === "send_document");
    assert(input, "initial capture was not transmitted");
    if (after) original.draft.edit(0, 0, after);
    await original.close();
    return { node, journal, who, options, input };
  };
  const words = (session) => session.draft.view.characters.map((one) => one.text).join("");
  const recovered = (session, input, body = "hello") => {
    const send = session.recoveredSends.find((one) => one.operation === input.mutation.operation);
    assert(send && send.text === body, "reopen did not restore the original body and operation controls");
    return send.sending;
  };
  return {
    async A4restoredNoDuplicate() {
      const { node, who, options, input } = await prepare("no-duplicate");
      const session = await ComposerSession.open(who, options); await clock.flush();
      try {
        const again = session.send(); let refused = false; void again.sent.catch(() => { refused = true; });
        await clock.flush();
        const transmissions = node.inputs.filter((one) => one.mutation?.do === "send_document");
        assert(transmissions.every((one) => JSON.stringify(one) === JSON.stringify(input)), "reopen and Send allocated a second operation for the original captured words");
        assert(refused && words(session) === "" && session.state.onTheirWay === 1, "reopened send did not retain its captured identities");
        const sending = recovered(session, input);
        node.complete(input); const updated = await sending.sent;
        assert(updated.landed && words(session) === "", "restored send did not reach its original terminal outcome");
        return { uniqueOperations: new Set(transmissions.map((one) => one.mutation.operation)).size, restoredControls: true, landed: true };
      } finally { await session.close(); node.source.destroy(); }
    },
    async A4restoredSuffix() {
      const { node, who, options, input } = await prepare("suffix", "later😀");
      const session = await ComposerSession.open(who, options); const sending = recovered(session, input); await clock.flush();
      try {
        assert(words(session) === "later😀", "restoring a seal hid text outside its captured identity range");
        node.complete(input); await sending.sent; await clock.flush();
        assert(words(session) === "later😀", "settling the restored capture removed later typing");
        return { captured: "hello", remaining: words(session) };
      } finally { await session.close(); node.source.destroy(); }
    },
    async A4restoredConcurrent() {
      const body = "中😀é";
      const { node, who, options, input } = await prepare("unicode-concurrent", "尾", body);
      node.source.getText("text").insert(0, "remote");
      const session = await ComposerSession.open(who, options); const sending = recovered(session, input, body);
      try {
        assert(words(session) === "remote尾", "restored identity ranges hid concurrent text or split a non-BMP scalar");
        node.complete(input); await sending.sent; await clock.flush();
        assert(words(session) === "remote尾", "settlement changed text outside the original captured identities");
        return { captured: body, remaining: words(session) };
      } finally { await session.close(); node.source.destroy(); }
    },
    async A4restoredFailure() {
      const { node, who, options, input } = await prepare("failure");
      const session = await ComposerSession.open(who, options); const sending = recovered(session, input);
      let failed = false; const answered = sending.sent.catch(() => { failed = true; });
      try {
        node.complete(input, "injected dispatch refusal"); await answered; await clock.flush();
        assert(failed && words(session) === "hello", "failed restored send did not show exactly the node-restored text");
        return { failureVisible: true, copiesInDraft: 1 };
      } finally { await session.close(); node.source.destroy(); }
    },
    async A4restoredWithdraw() {
      const { node, who, options, input } = await prepare("withdraw");
      node.behavior.mutate = async (request) => {
        if (request.mutation.do === "withdraw") node.publish({ kind: "result", revision: 1, operation: { operation: request.mutation.operation, accepted: false, landed: null, delivery: null, failure: null } });
      };
      const session = await ComposerSession.open(who, options); const sending = recovered(session, input);
      let failed = false; void sending.sent.catch(() => { failed = true; });
      try {
        const result = await sending.withdraw(); await clock.flush();
        assert(result.kind === "withdrawn" && failed && words(session) === "hello", "reopened confirmed withdrawal did not unseal its original text once");
        const count = node.inputs.filter((one) => one.mutation?.do === "send_document").length;
        await clock.advance(90_000);
        assert(node.inputs.filter((one) => one.mutation?.do === "send_document").length === count, "withdrawn restored send kept replaying");
        return { confirmedByNode: true, copiesInDraft: 1 };
      } finally { await session.close(); node.source.destroy(); }
    },
    async A4restoredAlreadySent() {
      const { node, who, options, input } = await prepare("already-sent");
      const session = await ComposerSession.open(who, options); const sending = recovered(session, input); let ended = false;
      void sending.sent.then(() => { ended = true; }, () => { ended = true; });
      try {
        const result = await sending.withdraw(); await clock.flush();
        assert(result.kind === "sent" && !ended && words(session) === "", "AlreadySent returned text or fabricated the original send's terminal outcome");
        node.complete(input); await sending.sent;
        return { wordsStayedSealed: true, waitedForOriginalOutcome: true };
      } finally { await session.close(); node.source.destroy(); }
    },
    async A4restoredDisconnect() {
      const { node, who, options, input } = await prepare("disconnect");
      const session = await ComposerSession.open(who, options); const sending = recovered(session, input); let ended = false;
      void sending.sent.then(() => { ended = true; }, () => { ended = true; });
      try {
        node.interrupt();
        const result = await sending.withdraw(); await clock.flush();
        assert(result.kind === "unavailable" && !ended && words(session) === "", "disconnect took back unresolved words without the node");
        return { unavailable: true, captureRetained: true };
      } finally { await session.close(); node.source.destroy(); }
    },
    async A4withdrawReopen() {
      const { node, who, options, input } = await prepare("withdraw-reopen");
      node.behavior.mutate = async () => {};
      const first = await ComposerSession.open(who, options); const takingBack = recovered(first, input).withdraw();
      await clock.flush();
      const withdrawal = node.inputs.find((one) => one.mutation?.do === "withdraw");
      assert(withdrawal, "withdrawal was not transmitted before closing");
      await first.close(); await takingBack;
      const sendsBefore = node.inputs.filter((one) => one.mutation?.do === "send_document").length;
      node.behavior.mutate = async (request) => {
        if (request.mutation.do === "withdraw") node.publish({ kind: "result", revision: 1, operation: { operation: request.mutation.operation, accepted: false, landed: null, delivery: null, failure: null } });
      };
      const session = await ComposerSession.open(who, options); const sending = recovered(session, input);
      try {
        let cancelled = false; try { await sending.sent; } catch { cancelled = true; }
        const requests = node.inputs.filter((one) => one.mutation?.do === "withdraw");
        assert(requests.every((one) => JSON.stringify(one) === JSON.stringify(withdrawal)), "withdrawal changed its persisted operation or number on reopen");
        assert(cancelled && words(session) === "hello" && node.inputs.filter((one) => one.mutation?.do === "send_document").length === sendsBefore, "restored withdrawal replayed the original send or failed to restore text");
        return { withdrawalIdentities: new Set(requests.map((one) => one.mutation.operation)).size, originalSendPaused: true };
      } finally { await session.close(); node.source.destroy(); }
    },
    async A4openingWithdrawOutcome() {
      const { node, who, options, input } = await prepare("opening-withdraw-outcome");
      node.behavior.mutate = async () => {};
      const first = await ComposerSession.open(who, options); const withdrawing = recovered(first, input).withdraw();
      await clock.flush(); const withdrawal = node.inputs.find((one) => one.mutation?.do === "withdraw");
      await first.close(); await withdrawing;
      const operation = node.complete(input); const opening = node.transport.watchDocuments;
      node.transport.watchDocuments = async (...args) => { const opened = await opening(...args); opened.frame.state.operations = [operation]; return opened; };
      node.behavior.mutate = async (request) => {
        if (request.mutation.do === "withdraw") node.publish({ kind: "refused", number: request.number, failure: { message: "these words were already sent" } });
      };
      const session = await ComposerSession.open(who, options); const sending = recovered(session, input);
      try {
        const result = await sending.sent; await clock.flush();
        assert(result.landed && words(session) === "", "pending withdrawal replaced the opening State's landed outcome with local cancellation");
        const requests = node.inputs.filter((one) => one.mutation?.do === "withdraw");
        assert(requests.every((one) => JSON.stringify(one) === JSON.stringify(withdrawal)), "opening receipt allocated a different withdrawal");
        return { originalOutcome: "landed", withdrawalIdentities: new Set(requests.map((one) => one.mutation.operation)).size };
      } finally { await session.close(); node.source.destroy(); }
    },
    async A4openingFailure() {
      const { node, who, options, input } = await prepare("opening-failure"); const opening = node.transport.watchDocuments;
      node.transport.watchDocuments = async (...args) => {
        const opened = await opening(...args);
        opened.frame.state.operations = [{ operation: input.mutation.operation, accepted: false, landed: null, delivery: null, failure: "injected opening refusal" }]; return opened;
      };
      const before = node.inputs.filter((one) => one.mutation?.do === "send_document").length;
      const session = await ComposerSession.open(who, options); const sending = recovered(session, input);
      try {
        let failed = false; try { await sending.sent; } catch { failed = true; }
        assert(failed && words(session) === "hello", "opening failure did not return original captured text once");
        assert(node.inputs.filter((one) => one.mutation?.do === "send_document").length === before, "opening refusal replayed the send");
        return { openingFailureConsumed: true, replayed: 0 };
      } finally { await session.close(); node.source.destroy(); }
    },
    async A2activeDowngrade() {
      const node = fixture(); const who = { resident: "composer-downgrade", machine: null };
      const session = await ComposerSession.open(who, { transport: node.transport, journal: memoryJournal(), recoveryKey: JSON.stringify([null, who.resident, "downgrade"]) });
      try {
        node.transport.watchDocuments = async () => null;
        const frame = node.frame(); frame.mutations = false; node.publish(frame);
        await session.reconnect();
        session.draft.edit(0, 0, "legacy"); await clock.flush();
        assert(node.inputs.length === 0 && node.legacy.some((one) => one.do === "edit"), "active downgrade kept a mixed native input path");
        return { entirelyLegacy: true };
      } finally { await session.close(); node.source.destroy(); }
    },
    async A2drainRefused() {
      const { NativeComposer } = product;
      const metadata = { revision: 1, draft: { id: "draft", revision: 0, epoch: 0, attachments: [], uploads: [], files_on: null }, queue: [], unobserved: [], commands: [], held: null, awaiting_turn: null };
      const native = new NativeComposer([]); native.receive({ kind: "state", version: 1, mutations: true, state: metadata, documents: [] });
      native.replica("draft").edit(0, 0, "retained offline text");
      const saved = { snapshot: native.snapshot(false), target: { kind: "draft" }, pending: [], contexts: [], history: [], retired: [], native: native.journal("draft") }; native.dispose();
      const node = fixture(""); node.transport.watchDocuments = async () => null;
      node.transport.syncText = async () => { throw new Error("injected legacy synchronization refusal"); };
      const who = { resident: "composer-drain-refused", machine: null };
      const session = await ComposerSession.open(who, { transport: node.transport, journal: memoryJournal([["native:replica:draft", saved]]), recoveryKey: JSON.stringify([null, who.resident, "drain-refused"]) });
      try {
        assert(!session.state.connected && session.state.error && words(session) === "retained offline text", "failed legacy drain lost local native text or concealed its failure");
        assert(node.inputs.length === 0 && node.legacy.length === 0, "failed drain started a mixed protocol path");
        return { textRetained: true, errorVisible: true };
      } finally { await session.close(); node.source.destroy(); }
    },
    async A4openingOutcome() {
      const { node, who, options, input } = await prepare("opening-outcome");
      const operation = node.complete(input); const opening = node.transport.watchDocuments;
      node.transport.watchDocuments = async (...args) => {
        const opened = await opening(...args); opened.frame.state.operations = [operation]; return opened;
      };
      const before = node.inputs.filter((one) => one.mutation?.do === "send_document").length;
      const session = await ComposerSession.open(who, options); const sending = recovered(session, input);
      try {
        const result = await sending.sent;
        assert(result.landed && node.inputs.filter((one) => one.mutation?.do === "send_document").length === before, "opening State's terminal receipt was lost or sent again");
        assert(session.state.onTheirWay === 0 && words(session) === "", "known opening outcome left a stale seal");
        return { openingReceiptConsumed: true, replayed: 0 };
      } finally { await session.close(); node.source.destroy(); }
    },
  };
}
