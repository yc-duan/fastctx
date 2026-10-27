import { Channel, invoke } from "@tauri-apps/api/core";
import { closeDelivery, observe } from "./delivery";
import type { Address } from "./wire";
import type { ComposerAttachmentAdded, ComposerMutation, ComposerPiece, ComposerUpdated, ComposerUploadScope, ComposerWatching } from "./composer";

/** One transport for desktop and mobile; the resident's node owns every draft and release. */
export interface ComposerTransport {
  read(): Promise<ComposerUpdated>;
  update(mutation: ComposerMutation): Promise<ComposerUpdated>;
  watch(onPiece: (piece: ComposerPiece) => void): Promise<ComposerWatching>;
  unwatch(id: number): Promise<void>;
  attachFile(scope: ComposerUploadScope, file: File): Promise<ComposerAttachmentAdded>;
  attachPath(scope: ComposerUploadScope, path: string, sent: (bytes: number) => void): Promise<ComposerAttachmentAdded>;
}

export function composing(who: Address): ComposerTransport {
  const address = { residentId: who.resident, machineName: who.machine };
  return {
    read: () => invoke("composer_read", address),
    update: (mutation) => invoke("composer_update", { ...address, mutation }),
    watch: (onPiece) => observe<ComposerPiece, ComposerWatching>("composer_watch", address, onPiece),
    unwatch: (id) => { closeDelivery(id); return invoke("unwatch", { id }); },
    attachFile: async (scope, file) => invoke("composer_attach_bytes", new Uint8Array(await file.arrayBuffer()), {
      headers: {
        resident: encodeURIComponent(who.resident), machine: encodeURIComponent(who.machine),
        operation: encodeURIComponent(scope.operation), target: encodeURIComponent(JSON.stringify(scope.target)),
        epoch: String(scope.epoch), name: encodeURIComponent(file.name), type: encodeURIComponent(file.type),
      },
    }),
    attachPath: (scope, path, sent) => {
      const pieces = new Channel<number>();
      pieces.onmessage = sent;
      return invoke("composer_attach_path", { ...address, operationId: scope.operation,
        target: scope.target, epoch: scope.epoch, path, sent: pieces });
    },
  };
}
