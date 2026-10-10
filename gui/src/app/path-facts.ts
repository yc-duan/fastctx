// What a terminal or Codex finds first on its PATH when it is not this
// computer's World program: a line to read, its paths and why on hover, and
// whether it is the old release whose unapply deletes the whole .fastctx.

import type { PathCommand } from "../world/renewal";
import { buildLabel } from "../world/renewal";
import type { SettingsWords } from "./words";

export interface PathFact {
  text: string;
  detail: string;
  legacy: boolean;
}

export function pathFacts(commands: PathCommand[] | undefined, words: SettingsWords): PathFact[] {
  return (commands ?? [])
    .filter((command) => !command.is_world)
    .map((command) => {
      const where = words.pathLookup[command.lookup];
      const text = command.is_world === null ? words.pathUnknown(where) : words.pathDifferent(where, command.build?.version ?? "?");
      const detail = [command.path, command.executable && command.executable !== command.path ? `→ ${command.executable}` : "", command.build ? buildLabel(command.build) : "", command.error ?? ""]
        .filter(Boolean)
        .join("\n");
      return { text, detail, legacy: command.legacy_unapply };
    });
}

/** The same situation, written once, so the notice is not shown again for it. */
export function pathSignature(facts: PathFact[]): string {
  return facts.map((fact) => `${fact.detail}|${fact.legacy}`).join(";");
}
