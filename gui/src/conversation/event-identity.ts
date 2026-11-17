import type { SessionEvent } from "../world/events";

/** Native object digests authenticate bytes; this also checks their row address. */
export function sameEvent(preview: SessionEvent, full: SessionEvent): boolean {
  if ((preview.event === "finished" || preview.event === "began" || preview.event === "updated") && preview.item.kind === "other" && ["Content preview unavailable", "fastctx:timeline-preview-unavailable"].includes(preview.item.host_kind)) return true;
  if (preview.event !== full.event) return false;
  if ((preview.event === "finished" || preview.event === "began" || preview.event === "updated") && (full.event === "finished" || full.event === "began" || full.event === "updated")) return preview.turn === full.turn && preview.item.id === full.item.id;
  if (preview.event === "heard" && full.event === "heard") return preview.turn === full.turn && preview.delivery === full.delivery && preview.message?.id === full.message?.id;
  if ("turn" in preview && "turn" in full) return preview.turn === full.turn;
  return true;
}
