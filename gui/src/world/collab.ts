// Model collaboration, as an account's setting (`world-gui.md`, 功能): which agents
// may call others, which they may call and how each is started, the text
// given to callers and the rules given to callees.

import { invoke } from "@tauri-apps/api/core";

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
}

export interface Collab {
  version: number;
  on: boolean;
  callers: AgentLogin[];
  callees: Callee[];
  /** The text given to callers; the World's default when null. */
  prompt: string | null;
  /** Given to every callee with its instructions. */
  rules: string;
}

export interface CollabView {
  collab: Collab;
  /** The World's default text for callers, to start editing from. */
  default_prompt: string;
}

/** An account's settings at a hub, or, with no account, this computer's
 *  own, which apply while it is in no network. */
export function readCollab(hub: string | null, account: string | null): Promise<CollabView> {
  return invoke("collab_read", { hub, account });
}

/** Writes the settings over the version they were read at; the new version.
 *  This computer's own are written only while it is in no network. */
export function writeCollab(hub: string | null, account: string | null, collab: Collab): Promise<number> {
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
