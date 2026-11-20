// The application was opened on this computer as an account (`fastctx data
// copies`): its node is told once a run, so the account's copies of the
// conversations living on other machines are taken in here — a machine the
// application was never opened on, a server, stops taking them in. Told
// only where the account is plain: the network shown has one login here;
// with several, none is told. An earlier node, or a device without one,
// answers otherwise, and nothing comes of it.

import { data } from "../world/data";
import type { WorldNow } from "../world/bridge";
import { shownNetwork } from "./network";

const told = new Set<string>();

export function noteOpened(now: WorldNow): void {
  const hub = shownNetwork(now);
  const seen = now.accounts.hubs.find((one) => one.hub === hub);
  if (!hub || !seen || seen.logins.length !== 1) return;
  const account = seen.logins[0];
  const opened = Date.parse(seen.accounts.find((one) => one.name === account)?.opened ?? "");
  if (!Number.isFinite(opened)) return;
  const key = `${hub}\n${account}\n${opened}`;
  if (told.has(key)) return;
  told.add(key);
  data({ do: "app_opened", hub, account, opened }).catch(() => {
    // An earlier node, or none here: there is nothing to keep.
  });
}
