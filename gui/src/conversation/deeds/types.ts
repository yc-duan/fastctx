// What a run did, read from its script and what it printed (`read.ts`): the
// things a person recognises at a glance, each drawn as itself
// (`draw.ts`). A command not recognised is only named; what it printed is
// in the run's own words, under them.

export interface TestFailure {
  name: string;
  /** What the test printed as it failed, as its runner gave it. */
  said: string[];
}

export interface TestsDeed {
  kind: "tests";
  command: string | null;
  cwd?: string | null;
  passed: number;
  failed: number;
  ignored: number;
  failures: TestFailure[];
}

export interface Diagnostic {
  level: "error" | "warning";
  code: string | null;
  message: string;
  place: { path: string; line: number; column: number } | null;
  /** The source and the compiler's marks under the message, as printed. */
  snippet: string[];
}

export interface ChecksDeed {
  kind: "checks";
  command: string | null;
  cwd?: string | null;
  items: Diagnostic[];
  /** Built and found nothing to say. */
  clean: boolean;
}

export interface CommitDeed {
  kind: "commit";
  command: string | null;
  branch: string;
  hash: string;
  subject: string;
  files: number | null;
  added: number | null;
  removed: number | null;
}

export type FileState = "modified" | "added" | "deleted" | "renamed" | "untracked" | "conflict";

export interface StatusDeed {
  kind: "status";
  command: string | null;
  branch: string | null;
  files: { state: FileState; path: string }[];
}

export interface LogDeed {
  kind: "log";
  command: string | null;
  commits: { hash: string; subject: string }[];
}

export interface PushDeed {
  kind: "push";
  command: string | null;
  remote: string | null;
  refs: { from: string; to: string; ok: boolean; why: string | null }[];
  /** Nothing was there to send. */
  current: boolean;
}

export interface SearchDeed {
  kind: "search";
  command: string | null;
  pattern: string | null;
  ignoreCase: boolean;
  files: { path: string; hits: { line: number | null; text: string }[] }[];
}

/** A line of a file as shown, or how many lines were passed over. */
export type FileRow = { line: number | null; text: string } | { skipped: number | null };

export interface FileDeed {
  kind: "file";
  command: string | null;
  machine: string | null;
  path: string;
  rows: FileRow[];
  /** How many lines the file has, where that was said. */
  of: number | null;
}

/** A copy between machines, as its heading said it. */
export interface CopyDeed {
  kind: "copy";
  command: string | null;
  from: { machine: string; path: string };
  to: { machine: string; path: string };
  files: number | null;
  /** How much came across, as the World wrote it ("1.2 MB"). */
  size: string | null;
  /** Files that were there already, the same. */
  same: number;
  /** What could not be copied, each as the command said it. */
  notCopied: string[];
}

/** A job that ended while it was waited for. */
export interface JobDeed {
  kind: "job";
  command: string | null;
  job: string;
  machine: string;
  /** How it ended, as the World wrote it ("exit 0", "stopped"). */
  how: string;
  failed: boolean;
  /** How long ago, where that was said. */
  ago: string;
  log: string | null;
  /** The last lines it wrote, where they followed. */
  tail: string[];
}

export interface StopDeed {
  kind: "stop";
  command: string | null;
  job: string;
  machine: string;
  how: string;
  stopped: boolean;
}

/** A page published, a version of it. */
export interface PageDeed {
  kind: "page";
  command: string | null;
  name: string;
  title: string;
  machine: string;
  version: number;
}

export interface ProjectDeed {
  kind: "project";
  command: string | null;
  town: string;
}

/** A picture a `view` gave: an image, or a page of a PDF drawn as one;
 *  `index` is which of the call's pictures it is. */
export interface PictureDeed {
  kind: "picture";
  command: string | null;
  path: string;
  machine: string | null;
  width: number;
  height: number;
  mime: string;
  page: number | null;
  index: number;
}

/** What the World's commands that settle things did. */
export interface WorldDeed {
  kind: "world";
  command: string | null;
  act: "plan" | "goal-complete" | "goal-blocked" | "goal-pause" | "sense";
  /** What a sentinel watches for. */
  said: string | null;
}

/** A command named and nothing more: what it printed is in the run's own
 *  words. */
export interface PlainDeed {
  kind: "plain";
  command: string;
}

/** Where among them the files the run wrote were written: their rows,
 *  each a fact of its own, go there. */
export interface WritesDeed {
  kind: "writes";
}

export type Deed = TestsDeed | ChecksDeed | CommitDeed | StatusDeed | LogDeed | PushDeed | SearchDeed | FileDeed | CopyDeed | JobDeed | StopDeed | PageDeed | ProjectDeed | PictureDeed | WorldDeed | PlainDeed | WritesDeed;

/** What a reader finds in a run's output: a deed, and the line it begins
 *  at, so the commands that printed them can be told in order. */
export interface Found<T extends Deed = Deed> {
  at: number;
  deed: T;
}

/** A run's output as lines; null where the lines were left out of what the
 *  model got. */
export type OutputLines = readonly (string | null)[];

export interface DeedWords {
  /** The run's own words, under what was drawn of them. */
  raw: string;
  tests(passed: number, failed: number, ignored: number): string;
  failed(count: number): string;
  /** Tests that passed, as a note under their card. */
  passed(count: number): string;
  /** Tests skipped. */
  skippedTests(count: number): string;
  /** Searches made, among other looking. */
  searches(count: number): string;
  /** Where a commit was sent. */
  pushedTo(branch: string): string;
  /** Commands run, gathered in one row. */
  commandsRun(count: number): string;
  /** The World's own doings, as their rows say them. */
  world: Record<"wait" | "stop" | "publish" | "enter" | "plan" | "sense" | "look", string>;
  /** What the plan is called on its row. */
  thePlan: string;
  /** A goal settled: the verb its row says, and what its card says. */
  goal: Record<"complete" | "blocked" | "pause", { verb: string; object: string; said: string }>;
  /** How long ago, from the World's English for it ("3 minutes ago"). */
  ago(said: string): string;
  /** How a job ended, from the World's word for it ("exit 1", "stopped"). */
  ended(how: string): string;
  version(count: number): string;
  page(count: number): string;
  /** What a copy brought across. */
  copied(files: number | null, size: string | null): string;
  same(count: number): string;
  notCopied(count: number): string;
  /** A run that went on after the call came back. */
  inBackground: string;
  /** A project entered. */
  entered: string;
  /** A sentinel set: it says when what it watches for comes. */
  watching: string;
  /** Other looking, gathered with files read and searches. */
  looked(count: number): string;
  errors(count: number): string;
  warnings(count: number): string;
  clean: string;
  files(count: number): string;
  noChanges: string;
  upToDate: string;
  rejected: string;
  hits(count: number, files: number): string;
  more(count: number): string;
  /** Lines of a file passed over where it was read. */
  skipped(count: number): string;
  lines(first: number, last: number, of: number | null): string;
  states: Record<FileState, string>;
  /** What each kind of deed is called on its row. */
  verbs: Record<"search" | "read" | "test" | "check" | "build" | "commit" | "push" | "status" | "log" | "run", string>;
  /** A status's or a history's subject where it names nothing itself. */
  tree: string;
  history: string;
  commits(count: number): string;
}
