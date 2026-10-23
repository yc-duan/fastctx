// A command as a frame names it: its program, and the script or the
// subcommand it was given.

import { wordsOf } from "../deeds/script";

export function program(command: string): string {
  const words = wordsOf(command);
  const first = leaf(words[0] ?? command);
  const second = words[1];
  if (!second || second.startsWith("-")) return first;
  return /[\\/]|\.[A-Za-z0-9]+$/.test(second) ? `${first} ${leaf(second)}` : `${first} ${second}`;
}

function leaf(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return cut < 0 ? trimmed : trimmed.slice(cut + 1);
}
