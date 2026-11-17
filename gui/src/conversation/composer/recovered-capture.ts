import * as Y from "yjs";
import type { Character } from "../../world/composer";
import type { DocumentSend } from "../../world/composer-sync";
import { decode } from "./bytes";
import { nativeId } from "./capture";

/** The persisted take is the original identity range. The body snapshot uses
 * fresh identities and must never be applied to the live draft to restore it. */
export function recoveredCapture(mutation: DocumentSend, visible: readonly Character[]): { characters: readonly Character[]; text: string } {
  if (!mutation.take || mutation.epoch === undefined) throw new Error("the retained send has no captured identity range");
  const deleted = Y.decodeUpdate(decode(mutation.take)).ds.clients;
  const characters = visible.filter((character) => {
    const id = nativeId(character.id);
    return deleted.get(id.client)?.some((range) => range.clock <= id.clock && id.clock + character.text.length <= range.clock + range.len);
  });
  const snapshot = new Y.Doc();
  try {
    Y.applyUpdate(snapshot, decode(mutation.snapshot));
    return { characters, text: snapshot.getText("text").toString() };
  } finally { snapshot.destroy(); }
}
