// Accounts on the hubs this computer uses, as the shell hands them over
// (`fastctx_types::accounts`, `::access`): who may use which machine, which
// machines an account trusts, the devices logged in, the projects, and who
// came into an account lately. Serialized replies, before any presentation.

import type { LinkState } from "./world";
import type { MachineName, Timestamp, TownName } from "./wire";

export type AccountName = string;
export type HubName = string;
export type HubId = string;

/** What an account may do on a machine (`fastctx_types::access::Rights`). */
export interface Rights {
  hands: boolean;
  agents: boolean;
}

export interface HubAccount {
  hub: HubId;
  account: AccountName;
}

/** Whom a conversation or a job works for (`fastctx_types::access::Acting`). */
export type Acting = { kind: "here" } | { kind: "accounts"; accounts: HubAccount[] };

export type MemberKind = "computer" | "phone" | "web";

/** A relay a machine connects through: a port forwarded to the hub. */
export interface Relay {
  label: string;
  addresses: string[];
}

export interface AccountSeen {
  name: AccountName;
  admin: boolean;
  may_register: boolean;
  locked: boolean;
  opened: Timestamp;
}

export interface MemberSeen {
  name: MachineName;
  kind: MemberKind;
  logins: AccountName[];
  /** The account it is registered under, when it is a machine. */
  home: AccountName | null;
  relays: Relay[];
  added: Timestamp;
  online: boolean;
  since: Timestamp | null;
  through: string | null;
  /** The start of its key, to tell two devices of one name apart. */
  key: string;
}

export interface GrantSeen {
  machine: MachineName;
  account: AccountName;
  rights: Rights;
}

export interface TrustSeen {
  account: AccountName;
  machine: MachineName;
}

export interface ProjectSeen {
  name: TownName;
  owner: AccountName;
  granted: AccountName[];
}

/** A device that logged in to an account, or a machine registered under
 *  it, not shown to the user yet. */
export interface Arrival {
  hub: HubId;
  account: AccountName;
  name: MachineName;
  device: boolean;
  noticed_at: Timestamp;
}

export interface HubAccounts {
  hub: HubId;
  name: HubName;
  link: LinkState;
  logins: AccountName[];
  home: AccountName | null;
  self_registration: boolean;
  accounts: AccountSeen[];
  members: MemberSeen[];
  grants: GrantSeen[];
  trust: TrustSeen[];
  projects: ProjectSeen[];
  arrivals: Arrival[];
}

export interface AccountsReply {
  machine: MachineName;
  hubs: HubAccounts[];
}

/** A change made as an account (`fastctx_types::accounts::AccountChange`). */
export type AccountChange =
  | { change: "grant"; machine: MachineName; to: AccountName; rights: Rights }
  | { change: "trust"; machine: MachineName; trusted: boolean }
  | { change: "relays"; machine: MachineName; relays: Relay[] }
  | { change: "remove"; name: MachineName; reason: string }
  | { change: "logout" }
  | { change: "token"; register: boolean; name: MachineName | null; kind: MemberKind; expires: Timestamp }
  | { change: "set_password"; target: AccountName; password: string }
  | { change: "open_account"; name: AccountName; admin: boolean; may_register: boolean; password: string }
  | { change: "set_account"; name: AccountName; admin: boolean; may_register: boolean; locked: boolean }
  | { change: "delete_account"; name: AccountName }
  | { change: "settings"; self_registration: boolean }
  | { change: "share_project"; town: TownName; with: AccountName; shared: boolean }
  | { change: "move_project"; town: TownName; to: AccountName }
  | { change: "seen"; name: MachineName };

/** One login of this computer or device: an account on a hub. */
export interface Login {
  hub: HubId;
  hubName: HubName;
  account: AccountName;
}

/** Every login, hub by hub, in the order the hubs answered. */
export function loginsOf(accounts: AccountsReply | null | undefined): Login[] {
  return (accounts?.hubs ?? []).flatMap((hub) => hub.logins.map((account) => ({ hub: hub.hub, hubName: hub.name, account })));
}

/** Whether a conversation working for `acting` works for `login`. */
export function actsFor(acting: Acting | undefined, login: Login): boolean {
  return !acting || acting.kind === "here" || acting.accounts.some((a) => a.hub === login.hub && a.account === login.account);
}
