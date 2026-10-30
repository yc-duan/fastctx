// What each machine's node keeps of its agents' logins: the JSON form of
// `fastctx_types::logins` (`world-agent-logins.md`). Reading it starts no
// process; a scan, a folder's search, a change or a fresh read of limits
// happens only when asked. A login is a directory; the World lists exactly
// the logins its tools are put into.

import type { AgentLimits, AgentModel, Fact, Integration } from "./agents";
import type { AgentName, MachineName, Timestamp } from "./wire";

/** Every machine asked: this one, or each the user may look at. */
export interface WorldLogins {
  machine: MachineName;
  machines: MachineLoginsHeard[];
}

export type MachineLoginsHeard =
  | { kind: "answered"; logins: MachineLogins }
  | { kind: "offline"; machine: MachineName; since: Timestamp | null }
  | { kind: "no_answer"; machine: MachineName; reason: string };

/** One machine's record. */
export interface MachineLogins {
  machine: MachineName;
  /** The scan running now, or the last one; absent before the first. */
  scan?: Scan;
  codex: AgentLogins;
  claude: AgentLogins;
  /** The folders the user chose there for logins kept below them. */
  folders?: Folder[];
}

/** One agent on a machine: its programs, and its logins in three groups. */
export interface AgentLogins {
  /** Its programs found here without running any; empty before a scan. */
  programs: FoundProgram[];
  /** The logins the World lists and uses: those its tools are put into. */
  listed: Login[];
  /** The others the usual places or the user gave; each can be put in. */
  candidates: Login[];
  /** Always empty: what a folder's search found is in the machine's
   *  `folders`. Kept for interfaces from before folders. */
  elsewhere: LooksLike[];
}

export interface Login {
  dir: string;
  /** The one the agent's programs use when nothing names another. */
  default: boolean;
  found: Found;
  /** Who is logged in, as the agent said when last asked; absent before. */
  identity?: Identified;
  /** Its usage windows as last read, with when; never read is unavailable. */
  limits: Fact<AgentLimits>;
  /** Indexes into the agent's `programs` of those that use this login:
   *  putting the World's tools in gives each of them `run`. */
  programs: number[];
  /** `in` for every listed login; a candidate is `out`, `old` where an old
   *  fastctx put its own in, `absent` where the program is not here. */
  integration: Integration;
  /** Claude Code: whether the World's conversations start without asking
   *  for approvals; granted with the tools. Read for listed logins only. */
  unattended?: boolean;
  /** Codex: whether the World names new conversations in it the way the
   *  official client does. On unless turned off. */
  auto_title?: boolean;
  /** The program the World starts for it. */
  launcher?: Launcher;
  /** The models a conversation in it can start on, as last read. */
  models?: AgentModel[];
  /** The change running in it now, or the last since the node started. */
  change?: LoginChange;
  /** What bringing the World's tools in it up to date waits for: none
   *  when they are as this fastctx writes them. */
  drift?: Drift;
}

/** Why the World's tools in a listed login were not brought up to date by
 *  themselves: something of theirs the user changed, or Codex's shared
 *  output limit, which only the user raises. */
export type Drift = { kind: "changed"; location: string; reason: string } | { kind: "asks"; limit: OutputLimit };

export type Found = "default" | "home" | "added" | "elsewhere";

export interface Identified {
  at: Timestamp;
  who: Fact<Identity>;
}

/** Who is logged in, as the agent says without its credentials read. */
export interface Identity {
  logged_in: boolean;
  /** Codex "chatgpt" or "apiKey"; Claude Code "claude.ai" (a /login
   *  subscription login) or "oauth_token" (a long-lived token from the
   *  environment, which names no account and no plan); others as named. */
  method?: string;
  email?: string;
  organization?: string;
  /** The plan as the agent names it: "plus", "pro", "max", …. */
  plan?: string;
}

export interface Launcher {
  path: string;
  /** As the program said when last started (Codex), or as its own records
   *  say (Claude Code). */
  version?: string;
  desktop_app: boolean;
}

/** A program found without running it. */
export interface FoundProgram {
  /** The command-line program; absent where its container holds none yet. */
  path?: string;
  how: Installed;
  /** The command-line program's own version, when its records say. */
  version?: string;
  /** The extension or desktop app it comes inside, with that one's own
   *  version, which is not the program's. */
  within?: Within;
  /** Whether typing the agent's command in a terminal here runs it. */
  on_path: boolean;
}

export type Installed =
  | "npm"
  | "pnpm"
  | "yarn"
  | "bun"
  | "official"
  | "winget"
  | "scoop"
  | "chocolatey"
  | "homebrew"
  | "desktop_app"
  | "extension"
  | "path";

export interface Within {
  /** "VS Code", "Cursor", "ChatGPT", "Claude", …. */
  name: string;
  /** The extension's or package's identifier: "openai.chatgpt". */
  id: string;
  version?: string;
  path: string;
}

export interface LooksLike {
  dir: string;
  /** The names in it that make it look like a login. */
  traces: string[];
}

export interface Scan {
  started_at: Timestamp;
  /** The usual places, each login there asked who it is, and the programs. */
  usual: Stage;
  /** Places that could not be looked into, and why. */
  unreadable?: string[];
}

/** A folder the user chose, for the logins kept below it. */
export interface Folder {
  dir: string;
  /** Its search, running or the last one; absent before the first. */
  search?: FolderSearch;
}

/** A search of everything below a folder, by the names in it. */
export interface FolderSearch {
  stage: Stage;
  /** Directories listed so far. */
  directories: number;
  /** Directories that could not be listed. */
  unreadable: number;
  /** What looks like a login below it that the World does not know yet;
   *  no one has asked who is logged in there. */
  found: FoundIn[];
}

export interface FoundIn {
  agent: AgentName;
  /** Relative to the folder. */
  path: string;
  /** The names in it that make it look like a login. */
  traces: string[];
}

/** What the user does with a folder: choose it (a folder that is a login
 *  becomes a candidate), stop its search, or no longer search it. */
export type FolderChange = { change: "choose" } | { change: "stop" } | { change: "forget" };

export type Stage =
  | { state: "running" }
  | { state: "done"; at: Timestamp }
  | { state: "stopped"; at: Timestamp }
  | { state: "failed"; at: Timestamp; reason: string };

/** Putting the World's tools into a login (`into`), or taking them out. */
export interface LoginChange {
  into: boolean;
  started_at: Timestamp;
  steps: ChangeStep[];
  /** Absent while it runs. */
  end?: ChangeEnd;
}

export interface ChangeStep {
  step: Step;
  state: StepState;
}

export type Step =
  | { step: "program" }
  | { step: "command"; command: string }
  | { step: "files"; changed?: string[] }
  | { step: "check"; scope: string };

export type StepState = { state: "running" } | { state: "done" } | { state: "failed"; reason: string };

export type ChangeEnd =
  | { end: "done"; at: Timestamp; kept?: Kept[] }
  | { end: "failed"; at: Timestamp; reason: string; output_limit?: OutputLimit };

export interface Kept {
  location: string;
  reason: string;
}

/** Codex's shared output limit, set lower than the World needs: raised only
 *  with the user's explicit word. */
export interface OutputLimit {
  location: string;
  current: number;
  requested: number;
}

/** What a change would do, worked out with nothing written. */
export interface ChangePreview {
  agent: AgentName;
  dir: string;
  into: boolean;
  files: FilePreview[];
  /** Claude Code's own commands it would run. */
  commands?: string[];
  /** Set when putting the tools in needs the output limit raised. */
  output_limit?: OutputLimit;
  /** What it leaves as the user has it. */
  kept?: Kept[];
}

export interface FilePreview {
  path: string;
  how: FileHow;
  items: ItemChange[];
}

export type FileHow = "created" | "changed" | "removed";

/** One item that would change; values only for what the World writes. */
export interface ItemChange {
  item: string;
  how: FileHow;
  before?: string;
  after?: string;
  /** The values exist but are not shown: they might be the user's. */
  withheld?: boolean;
}
