// What the World told a session, read back for a person. The World writes
// each notice for the model under a head note (`fastctx-hands/src/told.rs`);
// the forms it writes are recognised here and said in the interface's own
// words. Text in any other form is shown as it came.

export type Told =
  | { kind: "job"; job: string; machine: string; how: string; at: string; output: string }
  | { kind: "colleague"; colleague: string; machine: string; how: string; body: string }
  | { kind: "words"; from: string; body: string }
  | { kind: "plain"; body: string };

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
