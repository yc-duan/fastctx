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

/** Something the World gave up delivering, from the words it wrote for the
 *  model (`fastctx-node`'s `obligations.rs` and `on_stop.rs`): a command's
 *  end that can no longer come, or a stop that could not reach its machine,
 *  and why; null for words in any other form. */
export type Undelivered = { kind: "end" | "stop"; machine: string; why: UndeliveredWhy };

/** Why, in the forms the World writes; any other is kept as it came. */
export type UndeliveredWhy = { kind: "gone"; machine: string } | { kind: "left" } | { kind: "other"; text: string };

export function undelivered(text: string): Undelivered | null {
  const said = text.trim();
  const end = /^The end of j-[0-9a-z]+ on (\S+) cannot be delivered: (.+)\.$/.exec(said);
  const stop = /^The stop on (\S+) could not be delivered: (.+)\.$/.exec(said);
  const found = end ?? stop;
  if (!found) return null;
  return { kind: end ? "end" : "stop", machine: found[1]!, why: undeliveredWhy(found[2]!) };
}

function undeliveredWhy(why: string): UndeliveredWhy {
  const gone = /^(\S+) is no longer in this account's current network view$/.exec(why);
  if (gone) return { kind: "gone", machine: gone[1]! };
  if (why === "every network this work belongs to has been left or removed") return { kind: "left" };
  return { kind: "other", text: why };
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
