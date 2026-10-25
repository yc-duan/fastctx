// How words the node answered for stand under the user's bubble.
import { known, zhProblems } from "../app/problem";
import type { Landed } from "../world/bridge";

/** Whether `landed` stands as a line under the words. A conversation the
 *  World started brings its host back itself, so words that wait for that
 *  stay on their way, as any words sent (`world-lifecycle.md`, 用户看到的只是
 *  在发送); a wait the interface can name, as an allowance reached, is said. */
export function standsUnder(landed: Landed, heldByWorld: boolean): boolean {
  if (landed.kind === "sent" || landed.kind === "uncertain") return true;
  if (landed.kind !== "waiting") return false;
  return !heldByWorld || (landed.why !== null && known(landed.why, zhProblems) !== null);
}
