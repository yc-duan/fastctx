// The copies a specimen server holds of conversations living on other
// machines: its owner's paused since it was installed, many kept from
// before; another account's taken in. Counting and letting go answer after
// a moment, one copy kept as its conversation came to live there.

import type { CopiesAnswer, CopiesAsk, CopiesOf, CopyHeld } from "../world/data";

const MB = 1024 * 1024;

export function specimenCopies(): (machine: string, ask: CopiesAsk) => Promise<CopiesAnswer> {
  const accounts: CopiesOf[] = [
    { hub: "home", account: "me", opened: Date.UTC(2026, 3, 2), on: false, source: "install", paused_scopes: 142, paused_bytes: 6_240 * MB },
    { hub: "home", account: "ada", opened: Date.UTC(2026, 5, 14), on: true, source: "owner", paused_scopes: 0 },
  ];
  const after = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
  const held = (count: number): CopyHeld[] =>
    Array.from({ length: count }, (_, n) => ({ scope: `c/home/laptop/r${String(n).padStart(4, "0")}`, holding: 3 }));
  const report = (): CopiesAnswer => ({ copies: "report", accounts: accounts.map((one) => ({ ...one })) });
  return async (_machine, ask) => {
    await after(450);
    if (ask.do === "report") return report();
    const one = accounts.find((account) => account.hub === ask.hub && account.account === ask.account);
    if (!one) throw new Error(`no login here is of the account ${ask.account} at ${ask.hub}`);
    if (ask.do === "set") {
      one.on = ask.on;
      return report();
    }
    if (one.on) throw new Error(`the copies of ${ask.account} are taken in here: pause them first`);
    const counted = ask.only?.length ?? one.paused_scopes;
    const each = counted ? Math.round((one.paused_bytes ?? 0) / one.paused_scopes) : 0;
    if (!ask.apply) {
      return { copies: "cleared", applied: false, scopes: counted, bytes: counted * each, kept: [], keys: held(counted) };
    }
    const kept: [string, string][] = counted ? [["c/home/laptop/r0002", "its conversation lives here now"]] : [];
    const gone = counted - kept.length;
    one.paused_scopes -= gone;
    one.paused_bytes = Math.max(0, (one.paused_bytes ?? 0) - gone * each);
    return { copies: "cleared", applied: true, scopes: gone, bytes: gone * each, kept, keys: held(gone) };
  };
}
