// Accounts on the hubs this computer uses, as the shell hands them over
// (`fastctx_types::accounts`, `::access`): each account's level, the
// machines shared and the agents' logins lent between accounts, the
// machines of others whose agents may reach an account's machines, the
// devices logged in, the projects, and who came into an account lately.
// Serialized replies, before any presentation.

import type { LinkState } from "./world";
import type { MachineName, Timestamp, TownName } from "./wire";

export type AccountName = string;
export type HubName = string;
export type HubId = string;

/** What an account may do on a machine (`fastctx_types::access::Rights`):
 *  work on it, which brings every agent there; or use only the logins lent
 *  to it there. */
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

/** What an account is on its hub: the one super administrator, or a member. */
export type Level = "super" | "member";

export interface AccountSeen {
  name: AccountName;
  level: Level;
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
  /** Where it relays to the hub for others, when it is a relay. */
  relay_at?: string[];
  added: Timestamp;
  online: boolean;
  since: Timestamp | null;
  through: string | null;
  /** The start of its key, to tell two devices of one name apart. */
  key: string;
}

/** A machine shared with an account: working on it and every agent there. */
export interface ShareSeen {
  machine: MachineName;
  account: AccountName;
}

/** One login of an agent on a machine, lent to an account. */
export interface LendSeen {
  machine: MachineName;
  account: AccountName;
  agent: string;
  /** The login's directory as that machine writes it; null for the
   *  agent's default login there. */
  home: string | null;
}

/** A plan a login names none of (a Claude Code long-lived token), set by
 *  the hub's super administrator by hand; only shown. */
export type PlanLabel = "max" | "pro" | "free";

/** The plan set on one login of an agent on a machine. */
export interface PlanSeen {
  machine: MachineName;
  agent: string;
  /** The login's directory as that machine writes it; null for the
   *  agent's default login there. */
  home: string | null;
  plan: PlanLabel;
}

/** A machine of another whose agents may reach an account's machines,
 *  since the account first used one of them. */
export interface ActingSeen {
  machine: MachineName;
  account: AccountName;
  since: Timestamp;
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
  /** The hub's own addresses, as it publishes them. */
  addresses: string[];
  link: LinkState;
  logins: AccountName[];
  home: AccountName | null;
  self_registration: boolean;
  accounts: AccountSeen[];
  members: MemberSeen[];
  shares: ShareSeen[];
  lends: LendSeen[];
  /** Plans the super administrator set on logins that name none. */
  plans: PlanSeen[];
  acting: ActingSeen[];
  projects: ProjectSeen[];
  arrivals: Arrival[];
}

export interface AccountsReply {
  machine: MachineName;
  hubs: HubAccounts[];
}

/** A change made as an account (`fastctx_types::accounts::AccountChange`). */
export type AccountChange =
  | { change: "share"; machine: MachineName; to: AccountName; shared: boolean }
  | { change: "lend"; machine: MachineName; to: AccountName; agent: string; home?: string; lent: boolean }
  | { change: "plan"; machine: MachineName; agent: string; home?: string; plan: PlanLabel | null }
  | { change: "stop_acting"; machine: MachineName }
  | { change: "relays"; machine: MachineName; relays: Relay[] }
  | { change: "relay_at"; machine: MachineName; addresses: string[] }
  | { change: "remove"; name: MachineName; reason: string }
  | { change: "logout" }
  | { change: "token"; register: boolean; name: MachineName | null; kind: MemberKind; expires: Timestamp }
  | { change: "set_password"; target: AccountName; password: string }
  | { change: "open_account"; name: AccountName; password: string }
  | { change: "set_account"; name: AccountName; locked: boolean }
  | { change: "transfer"; to: AccountName }
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
