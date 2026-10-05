// What the World told a session, read back for a person. The World writes
// each notice for the model under a head note (`fastctx-hands/src/told.rs`);
// the forms it writes are recognised here and said in the interface's own
// words. Text in any other form is shown as it came.

export type Told =
  | { kind: "job"; job: string; machine: string; how: string; at: string; output: string }
  | { kind: "colleague"; colleague: string; machine: string; how: string; body: string }
  | { kind: "words"; from: string; body: string }
  | { kind: "plain"; body: string };

/** The user's own words as the World handed them to the session, under the
 *  head note it writes (`fastctx-hands/src/told.rs`). A host's history
 *  carries them so where the World has no record of handing them over, as
 *  in a conversation forked from another. */
export function userWords(text: string): { machine: string; body: string } | null {
  const said = /^=== from the user, on (\S+) ===\r?\n([\s\S]*)$/.exec(text);
  return said ? { machine: said[1]!, body: said[2]! } : null;
}

/** What the World itself said in words of its own (`fastctx-node`'s
 *  `management.rs` and `desk.rs`), recognised to be said again; null for
 *  words in any other form, shown as they came. */
export type WorldSaid =
  /** The user moved it to another project, or out of every project; the
   *  project's brief it was handed comes with it. */
  | { kind: "moved"; town: string | null; brief: string }
  /** Another resident's turn ended and Computer Use here is free. */
  | { kind: "desk_free"; who: string }
  /** Computer Use here is in another resident's turn. */
  | { kind: "desk_taken"; who: string }
  /** Computer Use here is held by a session outside the World. */
  | { kind: "desk_outside" };

export function worldSaid(body: string): WorldSaid | null {
  const lines = body.replace(/\r\n/g, "\n").split("\n");
  if (lines[0] === "=== World: the user changed your project ===") {
    const to = /^The user set your project to (\S+) through the World interface\./.exec(lines[1] ?? "");
    if (to) return { kind: "moved", town: to[1] === "none" ? null : to[1]!, brief: lines.slice(2).join("\n").trim() };
  }
  const free = /^(\S+)'s turn ended: this machine's Computer Use is free\.$/.exec(body);
  if (free) return { kind: "desk_free", who: free[1]! };
  const taken = /^(\S+) is using this machine's Computer Use in its turn; you will be told when that turn ends and the desk is free\.$/.exec(body);
  if (taken) return { kind: "desk_taken", who: taken[1]! };
  if (body.startsWith("This machine's Computer Use is held by a session outside the World")) return { kind: "desk_outside" };
  return null;
}

export function readTold(text: string): Told {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const first = lines[0] ?? "";
  const job = /^=== (j-[0-9a-z]+) on (\S+) ended \((.*)\) at (.+?); its output is in (.+) ===$/.exec(first);
  if (job && lines.length === 1) {
    return { kind: "job", job: job[1], machine: job[2], how: job[3], at: job[4], output: job[5] };
  }
  const colleague = /^=== colleague (\S+) on (\S+) \((.*)\) ===$/.exec(first);
  const last = lines[lines.length - 1] ?? "";
  if (colleague && lines.length >= 2 && /^=== what it (did|is doing): fastctx w \S+ ===$/.test(last)) {
    return {
      kind: "colleague",
      colleague: colleague[1],
      machine: colleague[2],
      how: colleague[3],
      body: lines.slice(1, -1).join("\n"),
    };
  }
  const words = /^=== from (.+) ===$/.exec(first);
  if (words) return { kind: "words", from: words[1], body: lines.slice(1).join("\n") };
  return { kind: "plain", body: text };
}
