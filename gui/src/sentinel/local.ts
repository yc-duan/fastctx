// A sentinel home is scoped by the hub that holds its registration. A bare
// name equal to this computer's global name can mean a remote machine in
// another hub. Without a scope in the record URL, every possible hub must
// resolve the name to this computer before local changes suffice.
import type { WorldNow } from "../world/bridge";

export function localHome(world: WorldNow | null, home: string): boolean {
  if (!world?.nodes.facts) return false;
  const hubs = world.accounts.hubs.filter((hub) => hub.logins.length > 0);
  return hubs.length > 0 && hubs.every((hub) => {
    const member = hub.members.find((member) => member.name === home || member.was?.includes(home));
    const me = hub.members.find((member) => member.name === hub.me || (!!hub.me && member.was?.includes(hub.me)));
    return !!member && member === me;
  });
}
