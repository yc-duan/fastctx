// The application was opened on this computer as an account (`fastctx data
// copies`): its node is told, so the account's copies of the conversations
// living on other machines are taken in here — a machine the application
// was never opened on, a server, stops taking them in. Told only where the
// account is plain: the network shown has one login here; with several,
// none is told. Told once a run once the node took it; one it did not take —
// not there yet, an earlier node, or a device with none — is told again as
// the World comes again, a minute apart at least.

import { data } from "../world/data";
import type { WorldNow } from "../world/bridge";
import { shownNetwork } from "./network";

const told = new Set<string>();
const telling = new Set<string>();
const refused = new Map<string, number>();
const AGAIN_MS = 60_000;

export function noteOpened(now: WorldNow): void {
  const hub = shownNetwork(now);
  const seen = now.accounts.hubs.find((one) => one.hub === hub);
  if (!hub || !seen || seen.logins.length !== 1) return;
  const account = seen.logins[0];
  const opened = Date.parse(seen.accounts.find((one) => one.name === account)?.opened ?? "");
  if (!Number.isFinite(opened)) return;
  const key = `${hub}\n${account}\n${opened}`;
  if (told.has(key) || telling.has(key)) return;
  const last = refused.get(key);
  if (last !== undefined && Date.now() - last < AGAIN_MS) return;
  telling.add(key);
  data({ do: "app_opened", hub, account, opened })
    .then(() => told.add(key))
    .catch(() => {
      // Not taken now: told again a while later as the World comes again.
      refused.set(key, Date.now());
    })
    .finally(() => telling.delete(key));
}
