// Model collaboration, as an account's setting (`world-gui.md`, 功能): which agents
// may call others, and the presets they may call: what each is asked for,
// how it is started, the text its callers are given and the rules it is
// given.

import { invoke } from "@tauri-apps/api/core";
import { inShell } from "./bridge";

/** One login of an agent on a machine; `home` is its directory, or null
 *  for the agent's default login there. */
export interface AgentLogin {
  machine: string;
  agent: "codex" | "claude";
  home: string | null;
}

export interface Callee {
  /** What the calling model names it by: lowercase letters, digits and
   *  hyphens, unique in the account. */
  name: string;
  /** One line on when to call it. */
  use_when: string;
  login: AgentLogin;
  model: string | null;
  effort: string | null;
  /** Whether it may operate that machine's screen. */
  screen: boolean;
  /** How many of its colleagues may be at work at once; absent, the
   *  default of four. */
  at_once?: AtOnce;
  /** What it is asked for; absent as a client from before presets wrote
   *  it, which is the World's reviewer with the account's text for
   *  callers. */
  preset?: Preset;
}

/** Which of the World's presets a callee is, or the account's own. */
export type PresetKind = "review" | "scout" | "own";

/** The World's presets, in the order they are shown. */
export const WORLD_KINDS: PresetKind[] = ["review", "scout"];

export interface Preset {
  /** A kind a later release names is read as the account's own. */
  kind: PresetKind | (string & {});
  /** Its text for callers; absent, the World's for its kind. */
  prompt?: string | null;
  /** What the account adds to its rules; for one of the account's own,
   *  what it does. */
  rules?: string;
}

/** The World's texts for one of its presets. */
export interface PresetTexts {
  kind: PresetKind | (string & {});
  /** What it does, as callers read under its name. */
  does: string;
  /** Its text for callers, to start rewriting from and to put back. */
  prompt: string;
}

/** The World's text for callers as it reads for a callee in `agent`'s host,
 *  as the node words it (`collab_presets::in_host`): written of GPT, run by
 *  Codex, it names Claude, run by Claude Code, for a login of Claude Code's.
 *  An account's own text is never put through this. */
export const inHost = (text: string, agent: string): string =>
  agent === "claude"
    ? text.replaceAll("GPT, run by Codex", "Claude, run by Claude Code").replaceAll("unless you are GPT too", "unless you are Claude too")
    : text;

/** The preset `callee` is. */
export const kindOf = (callee: Callee): PresetKind => {
  const kind = callee.preset?.kind;
  return kind === undefined || kind === "review" ? "review" : kind === "scout" ? "scout" : "own";
};

/** A callee's limit: at most so many at once, or none. */
export type AtOnce = { most: number } | "any";

/** What a callee whose settings say nothing works with. */
export const AT_ONCE_DEFAULT = 4;

/** What a conversation's colleagues of every preset together work within,
 *  where the settings say nothing. */
export const AT_ONCE_IN_ALL_DEFAULT = 8;

export interface Collab {
  version: number;
  on: boolean;
  callers: AgentLogin[];
  /** The presets turned on: what callers are offered. */
  callees: Callee[];
  /** The account's text for callers of a callee written before presets;
   *  the World's when null. */
  prompt: string | null;
  /** Given to every callee after its own rules. */
  rules: string;
  /** The presets kept while turned off; absent as a client from before
   *  presets writes the settings. */
  shelved?: Callee[];
  /** How many colleagues one conversation may have at work at once, of
   *  every preset together; absent, the default of eight. */
  at_once?: AtOnce;
}

export interface CollabView {
  collab: Collab;
  /** The World's text for the callers of its reviewer. */
  default_prompt: string;
  /** The World's presets; absent from a release before them. */
  presets?: PresetTexts[];
  /** Whether where the settings are kept keeps presets: a hub from before
   *  them would write a preset's kind and texts, and those kept while off,
   *  away. */
  keeps_presets?: boolean;
}

/** An account's settings at a hub, or, with no account, this computer's
 *  own, which apply while it is in no network. */
export function readCollab(hub: string | null, account: string | null): Promise<CollabView> {
  if (import.meta.env.DEV && !inShell()) return import("../specimen/collab").then((specimen) => (shown ??= specimen.specimenCollab()));
  return invoke("collab_read", { hub, account });
}

/** The specimen's settings, as last written. */
let shown: CollabView | null = null;

/** Writes the settings over the version they were read at; the new version.
 *  This computer's own are written only while it is in no network. */
export function writeCollab(hub: string | null, account: string | null, collab: Collab): Promise<number> {
  if (import.meta.env.DEV && !inShell()) {
    const version = collab.version + 1;
    if (shown) shown = { ...shown, collab: { ...collab, version } };
    return Promise.resolve(version);
  }
  return invoke("collab_write", { hub, account, collab });
}

/** A login's directory as compared: slashes alike and no trailing one;
 *  case counts except in a Windows path, by its drive. */
const homeKey = (home: string | null): string => {
  const plain = (home ?? "").replace(/\\/g, "/").replace(/\/+$/, "");
  return /^[A-Za-z]:\//.test(plain) ? plain.toLowerCase() : plain;
};

export const sameAgentLogin = (a: AgentLogin, b: AgentLogin): boolean =>
  a.machine === b.machine && a.agent === b.agent && homeKey(a.home) === homeKey(b.home);
