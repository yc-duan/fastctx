// The World's tools as a page calls them, and what each answers: the page's
// side of the bridge. It is a contract of its own, simpler and steadier
// than the World's wire, since pages are written against it and kept: the
// host turns each call into the World's calls and the answers into these
// shapes. Reading tools need nothing; the doing tools are done only where
// the page's version declared them and the user agreed (`specs/world-gui.md`,
// 软件分两层).

export type AgentState = "working" | "waiting" | "idle" | "asleep" | "offline";

export interface Tools {
  /** Who is in the World now and on which machines. */
  "world.now": {
    args: Record<string, never>;
    answer: {
      machines: { name: string; online: boolean; system: string | null; device: boolean }[];
      agents: {
        resident: string;
        machine: string;
        agent: string;
        project: string | null;
        title: string | null;
        state: AgentState;
        /** Since when it has been in that state. */
        since: string | null;
        /** What it did last, said as the World says it. */
        last: string | null;
      }[];
    };
  };
  /** The page's own project: what it is and what it keeps. */
  "project.map": {
    args: Record<string, never>;
    answer: {
      project: string;
      name: string;
      what: string;
      machines: string[];
      pages: { page: string; title: string; description: string | null }[];
    };
  };
  /** A file on a machine, whole or its end. */
  "file.read": {
    args: { machine: string; path: string; tail?: number };
    answer: { text: string | null; size: number; modified: string; offset: number };
  };
  /** A job, running or ended: how it stands, and the end of what it wrote.
   *  `exit` is the shell's exit code once it has one; `signal` is set
   *  instead when a Unix signal ended it. */
  "job.read": {
    args: { machine: string; job: string };
    answer: {
      job: string;
      command: string;
      state: "running" | "exited" | "stopped" | "interrupted" | "failed";
      exit: number | null;
      signal: number | null;
      started: string;
      output: string;
    };
  };
  /** Opens an agent's conversation in the interface, leaving the page;
   *  nothing is done in the World. */
  open: {
    args: { resident: string; machine: string };
    answer: Record<string, never>;
  };
  /** Runs a command the page declared, on a machine, as the page. */
  run: {
    args: { machine: string; command: string };
    answer: { exit: number | null; output: string };
  };
  /** Says words to an agent the page declared, marked as the page's. */
  say: {
    args: { resident: string; machine: string; text: string };
    answer: { delivered: boolean };
  };
}

export type ToolName = keyof Tools;
