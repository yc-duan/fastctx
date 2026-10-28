// Accounts on the hubs this computer uses, as the shell hands them over
// (`fastctx_types::accounts`, `::access`): each account's level, the
// machines shared and the agents' logins assisting other accounts, the
// machines of others whose agents may reach an account's machines, the
// devices logged in, the projects, and who came into an account lately.
// Serialized replies, before any presentation.

import type { LinkState } from "./world";
import type { MachineName, Timestamp, TownName } from "./wire";

export type AccountName = string;
export type HubName = string;
export type HubId = string;

/** What an account may do on a machine (`fastctx_types::access::Rights`):
 *  work on it, which brings every agent there; or use only the logins
 *  assisting it there. */
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

/** A relay a machine connected through before direct connections replaced
 *  relays; only change records made then still name one. */
export interface Relay {
  label: string;
  addresses: string[];
}

/** What the network's administrator chooses a machine's direct address
 *  from: its network cards' addresses, and where the hub sees it. */
export interface Reach {
  /** None while it reaches the hub through another machine. */
  seen?: string;
  interfaces?: string[];
}

/** One machine's direct link with another. */
export interface LinkSeen {
  peer: MachineName;
  /** Whether it is up; when not, what goes between the two goes through
   *  the hub. */
  up: boolean;
  /** Its round trip in milliseconds while up, as last measured. */
  ms?: number;
  /** When it came up or went down. */
  since: Timestamp;
  /** Why it is down, or why the last try to bring it up failed. */
  why?: string;
}

/** How a machine's lines go, as it last told the hub. */
export interface Paths {
  /** The round trip of its line to the hub, in milliseconds. */
  hub_ms?: number;
  links?: LinkSeen[];
}

/** What an account is on its hub (`world-network.md`, 一个中枢，几个账号):
 *  the one super administrator; a member, whose machines the super
 *  administrator may use; or a visitor, whose machines are its alone. */
export type Level = "super" | "member" | "visitor";

export interface AccountSeen {
  name: AccountName;
  level: Level;
  locked: boolean;
  opened: Timestamp;
  /** Made a member by the super administrator, waiting for the account to
   *  agree. */
  promotion?: boolean;
}

export interface MemberSeen {
  name: MachineName;
  /** The names it had before it was renamed, oldest first; each still
   *  names it. */
  was?: MachineName[];
  kind: MemberKind;
  logins: AccountName[];
  /** The account it is registered under, when it is a machine. */
  home: AccountName | null;
  /** Where other machines connect to it directly, when the network's
   *  administrator allowed that. */
  direct?: string[];
  /** Where it relays its owner's other machines to the hub, when its owner
   *  let it. */
  relay?: string[];
  added: Timestamp;
  online: boolean;
  since: Timestamp | null;
  /** The machine it reaches the hub through, when its own line is down. */
  through: string | null;
  /** What its direct address may be chosen from: for the administrator,
   *  while it is online. */
  reach?: Reach;
  /** How its lines go, as it last said, while it is online. */
  paths?: Paths;
  /** The start of its key, to tell two devices of one name apart. */
  key: string;
  /** A managed machine: registered under `home`, logged in to nothing
   *  (`world-network.md`, 受管机器). */
  managed?: boolean;
  /** The code it came in by, when one did. */
  code?: string;
  /** The group its owner put it in, which only says how it is shown
   *  (`world-network.md`, 机器的分组). */
  group?: string;
}

/** A machine shared with an account: working on it and every agent there. */
export interface ShareSeen {
  machine: MachineName;
  account: AccountName;
  /** The most jobs the account runs on it at once; absent is no limit. */
  jobs?: number;
}

/** One login of an agent on a machine, assisting an account. */
export interface AssistSeen {
  machine: MachineName;
  account: AccountName;
  agent: string;
  /** The login's directory as that machine writes it; null for the
   *  agent's default login there. */
  home: string | null;
  /** When the assistance ends by itself; null while the owner keeps it on. */
  until: Timestamp | null;
  /** What the account may spend of the login in a week, in millionths of a
   *  US dollar; absent is no limit. */
  weekly?: number;
  /** What it may spend in any 24 hours, the same way; absent is no limit. */
  daily?: number;
  /** The share of the login's own five-hour window, in percent, past which
   *  the login assists no one; absent is no limit. */
  five_hour?: number;
  /** The most turns the login runs at once, its owner's own counted too;
   *  absent leaves it to the vendor's rate limits. */
  turns?: number;
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
/** What another did to a machine this account holds, told to its holders
 *  once (`world-network.md`, 共享机器与智能体协助). `by` is none where the hub
 *  itself took back a share of the machine it runs on. */
export interface MachineNotice {
  id: number;
  at: Timestamp;
  account: AccountName;
  machine: MachineName;
  by: AccountName | null;
  action:
    | { kind: "rename"; to: MachineName }
    | { kind: "updated"; from: string; to: string }
    | { kind: "remove"; then: "uninstall" | "everything" | null }
    | { kind: "share"; to: AccountName; shared: boolean }
    | { kind: "assist"; to: AccountName; agent: string; home: string | null; assisting: boolean; until: Timestamp | null };
}

export interface Arrival {
  hub: HubId;
  account: AccountName;
  name: MachineName;
  device: boolean;
  noticed_at: Timestamp;
  /** The code a managed machine came in by: those of one code are told
   *  together. */
  code?: string;
}

/** How a code reaches the devices it lets in, for whoever made it to know
 *  it again: a phone scans it, a Linux host runs the line it is in, a
 *  computer has it pasted, or this computer's SSH session with a host uses
 *  it at once. A way a later release names reads as `other`. */
export type TokenWay = "scan" | "line" | "paste" | "ssh" | "other";

/** When a code that lasts until it is revoked is said to expire. */
export const LASTING = "9999-12-30T00:00:00Z";

/** Whether a code expiring at `expires` lasts until it is revoked. */
export function lasting(expires: Timestamp): boolean {
  return new Date(expires).getTime() >= Date.parse(LASTING);
}

/** A code that still lets devices in, as the accounts it lets into see it,
 *  and the network's administrator. */
export interface TokenSeen {
  /** Its key, by which it is revoked. */
  key: string;
  account: AccountName;
  label?: string;
  kind: MemberKind;
  managed?: boolean;
  /** How many devices may use it: any number at zero. */
  uses: number;
  /** How many have. */
  used: number;
  /** At `LASTING`, never. */
  expires: Timestamp;
  /** How it is given; an earlier hub does not say. */
  way?: TokenWay;
  /** The name the one device it is for takes, when it names one. */
  name?: MachineName;
  /** The device that made it, and when; an earlier hub does not say. */
  made_by?: MachineName;
  made_at?: Timestamp;
}

/** What a code lets this computer into, as its hub told before it is used. */
export interface TokenPeek {
  hub: HubName;
  account: AccountName;
  /** It makes this computer a managed machine of `account`. */
  managed: boolean;
  label?: string;
  expires: Timestamp;
  /** Why this computer may not come in by it, when it may not. */
  refused?: Unfit;
}

/** Why this World may not become a managed machine: it starts from one
 *  never used, on no other network. */
export type Unfit = { why: "network" } | { why: "conversations" } | { why: "integrated" } | { why: "unknown"; detail: string };

export interface HubAccounts {
  hub: HubId;
  name: HubName;
  /** The hub's own addresses, as it publishes them. */
  addresses: string[];
  link: LinkState;
  /** This computer's name there: its name, unless that hub has not
   *  followed a rename yet. Absent from an older node. */
  me?: MachineName;
  /** Whether the hub renames machines. */
  renames?: boolean;
  /** Whether the hub registers managed machines (`world-network.md`, 受管机器). */
  managed?: boolean;
  /** Whether the hub has visitors (`world-network.md`, 一个中枢，几个账号). */
  roles?: boolean;
  /** Whether the hub keeps the groups of machines (`world-network.md`,
   *  机器的分组). */
  groups?: boolean;
  /** Whether the hub dissolves its network when its super administrator
   *  asks (`world-network.md`, 解散网络). */
  dissolves?: boolean;
  /** Whether the hub deletes a project its owner asks it to. */
  deletes_projects?: boolean;
  /** Whether the hub keeps the machines that relay for their owner's others
   *  on their network. */
  lan_relays?: boolean;
  /** Whether the hub keeps the network's versions and their policy. */
  versions?: boolean;
  package_progress?: boolean;
  /** The hub keeps updates for machines away until they are back. */
  due?: boolean;
  /** The hub tells a machine it takes off to uninstall Fastctx. */
  uninstalls?: boolean;
  logins: AccountName[];
  home: AccountName | null;
  self_registration: boolean;
  accounts: AccountSeen[];
  members: MemberSeen[];
  shares: ShareSeen[];
  /** The machine this hub runs on, when it is known: it is shared with no
   *  one (`world-network.md`, 共享机器与智能体协助). */
  hub_machine?: MachineName;
  /** What others did to machines this device's accounts hold. */
  machine_notices?: MachineNotice[];
  assists: AssistSeen[];
  /** Plans the super administrator set on logins that name none. */
  plans: PlanSeen[];
  acting: ActingSeen[];
  projects: ProjectSeen[];
  arrivals: Arrival[];
  /** This computer's direct links with the machines there, as it holds
   *  them now. */
  links?: LinkSeen[];
  /** The codes this computer's logins there made that still let devices in. */
  tokens?: TokenSeen[];
}

export interface AccountsReply {
  machine: MachineName;
  /** The names this computer had, oldest first. */
  was?: MachineName[];
  hubs: HubAccounts[];
  /** The networks this computer was on that were dissolved, not yet seen. */
  dissolved?: Dissolution[];
}

/** A network this computer was on, dissolved by its administrator: the
 *  computer left it, and says so once. */
export interface Dissolution {
  hub: HubName;
  at: Timestamp;
}

/** A change made as an account (`fastctx_types::accounts::AccountChange`). */
export type AccountChange =
  | { change: "share"; machine: MachineName; to: AccountName; shared: boolean; jobs?: number }
  | { change: "assist"; machine: MachineName; to: AccountName; agent: string; home?: string; assisting: boolean; until?: Timestamp; weekly?: number; daily?: number; five_hour?: number; turns?: number }
  | { change: "plan"; machine: MachineName; agent: string; home?: string; plan: PlanLabel | null }
  | { change: "stop_acting"; machine: MachineName }
  /** The administrator's change: the server checks it reaches the machine
   *  at `at` before it counts; none, no longer. */
  | { change: "direct"; machine: MachineName; at: string[] }
  /** Takes a machine off; `then` has it uninstall Fastctx as it hears so. */
  | { change: "remove"; name: MachineName; reason: string; then?: "uninstall" | "everything" }
  /** Renames one of the account's machines, or, for the administrator,
   *  any; its names before keep naming it. */
  | { change: "rename"; machine: MachineName; to: MachineName }
  /** Puts one of the account's machines (or, for the administrator, a
   *  member's) in a group, or in none; renames a group, or ends it. */
  | { change: "group"; machine: MachineName; group: string | null }
  | { change: "rename_group"; owner: AccountName; from: string; to: string | null }
  | { change: "logout" }
  /** A code for another device; `managed` makes the computers that use it
   *  managed machines, `uses` of them at most (any number at zero, one when
   *  absent); expiring at `LASTING`, it lasts until it is revoked. */
  | { change: "token"; register: boolean; name: MachineName | null; kind: MemberKind; expires: Timestamp; managed?: boolean; uses?: number; label?: string; way?: TokenWay }
  | { change: "revoke_token"; token: string }
  | { change: "set_password"; target: AccountName; password: string }
  | { change: "open_account"; name: AccountName; password: string; visitor?: boolean }
  /** The super administrator makes `name` a visitor at once, or a member
   *  once the account agrees. */
  | { change: "set_role"; name: AccountName; visitor: boolean }
  | { change: "accept_role"; accept: boolean }
  | { change: "set_account"; name: AccountName; locked: boolean }
  | { change: "transfer"; to: AccountName }
  | { change: "delete_account"; name: AccountName }
  | { change: "settings"; self_registration: boolean }
  | { change: "updates"; policy: import("./updates").Policy }
  /** A machine away updates to `version` once it is back; none takes
   *  that back. */
  | { change: "update_due"; machine: MachineName; version?: string; batch?: string }
  /** The super administrator dissolves the network named `name`, typed as
   *  it is. */
  | { change: "dissolve"; name: HubName }
  | { change: "share_project"; town: TownName; with: AccountName; shared: boolean }
  | { change: "move_project"; town: TownName; to: AccountName }
  /** The owner deletes a project and its ledger, on a hub that says it
   *  deletes projects. */
  | { change: "delete_project"; town: TownName }
  /** The owner lets one of their machines relay their others to the hub at
   *  addresses on its own network, or, with none, no longer. */
  | { change: "relay"; machine: MachineName; at: string[] }
  | { change: "seen"; name: MachineName };

/** One login of this computer or device: an account on a hub. */
export interface Login {
  hub: HubId;
  hubName: HubName;
  account: AccountName;
}

/** Whether `name` names `member`: its name now, or one it had. */
export function names(member: Pick<MemberSeen, "name" | "was">, name: MachineName): boolean {
  return member.name === name || (member.was ?? []).includes(name);
}

/** Every login, hub by hub, in the order the hubs answered. */
export function loginsOf(accounts: AccountsReply | null | undefined): Login[] {
  return (accounts?.hubs ?? []).flatMap((hub) => hub.logins.map((account) => ({ hub: hub.hub, hubName: hub.name, account })));
}

/** Whether a conversation working for `acting` works for `login`. */
export function actsFor(acting: Acting | undefined, login: Login): boolean {
  return !acting || acting.kind === "here" || acting.accounts.some((a) => a.hub === login.hub && a.account === login.account);
}
