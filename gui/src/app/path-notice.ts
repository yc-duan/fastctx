// PATH is a local maintenance fact, available even before World login.

import { escape } from "../conversation/prose";
import type { RenewalStatus } from "../world/renewal";
import { Sheet } from "./confirm";
import type { SettingsWords } from "./words";

export function showPathNotice(status: RenewalStatus, words: SettingsWords, close: string): void {
  const commands = (status.commands ?? []).filter(command => !command.is_world);
  if (!commands.length) return;
  const sheet = new Sheet("confirm-sheet");
  const notes = commands.flatMap(command => [
    command.is_world === null ? words.pathUnknown(command.lookup, command.path, command.error ?? "?")
      : words.pathDifferent(command.lookup, command.path, command.build?.version ?? "?", command.executable ?? command.error ?? "?"),
    command.legacy_unapply ? words.pathLegacy : "",
  ]).filter(Boolean);
  sheet.body.innerHTML = `${sheet.head(words.worldVersion, close)}${notes.map(note => `<p class="confirm-what">${escape(note)}</p>`).join("")}`;
  sheet.show();
}
