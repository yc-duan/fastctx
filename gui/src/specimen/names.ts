// What a specimen says of a few names typed into it: the shell's rule
// answers in the app (`src-tauri/src/names.rs`); a page in a plain browser
// has no shell, so these are that rule's answers for these names, as it
// gave them, and every other name passes.

import type { NameProblem } from "../world/bridge";

const answers: Record<string, NameProblem> = {
  "machine:台 式机": { problem: "space" },
  "machine:pаypal": { problem: "mixed_scripts", first: "p", second: "а" },
  "machine:gpu.home": { problem: "not_allowed", character: "." },
  "machine:PC-Ελλάδα": { problem: "mixed_scripts", first: "P", second: "Ε" },
  "machine:一二三四五六七八九十甲乙丙丁戊己庚辛壬癸子丑": { problem: "too_long" },
  "machine:-gpu": { problem: "bad_start", character: "-" },
  "machine:hub": { problem: "reserved" },
  "hub:我的 家": { problem: "space" },
  "hub:家.": { problem: "not_allowed", character: "." },
};

export async function specimenNameProblem(kind: "machine" | "hub", text: string): Promise<NameProblem | null> {
  return answers[`${kind}:${text}`] ?? null;
}
