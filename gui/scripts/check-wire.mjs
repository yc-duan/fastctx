// Check frozen Rust JSON as fresh TS literals, so excess fields, discriminants,
// explicit nulls and omitted required fields are checked without type assertions.
// --rust also checks the fixture against serde; --write regenerates it first.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { build } from "esbuild";

const gui = fileURLToPath(new URL("../", import.meta.url));
const fixturePath = path.join(gui, "fixtures/wire.json");
const flags = process.argv.slice(2);
assert(flags.every((flag) => flag === "--rust" || flag === "--write"), "expected --rust or --write");
if (flags.length) {
  const generated = execFileSync("cargo", ["run", "--locked", "--quiet", "-j", "4", "--manifest-path", path.join(path.dirname(gui), "Cargo.toml"), "-p", "fastctx-types", "--example", "gui_wire_fixtures"], {
    cwd: path.dirname(gui), encoding: "utf8", stdio: ["ignore", "pipe", "inherit"],
  });
  if (flags.includes("--write")) writeFileSync(fixturePath, generated);
  // Compare text: JSON.parse cannot preserve all of Rust's u64 integer values.
  assert.equal(readFileSync(fixturePath, "utf8"), generated, "wire fixture is stale; run npm run fixtures:wire");
}
const json = readFileSync(fixturePath, "utf8");
const fixture = JSON.parse(json);
const virtualFile = path.join(gui, "fixtures/contract.ts");
const source = `
import type { SessionEvent, Watched, SessionChange, Health } from "../src/world/events";
import type { WorldWho, NodesReply, TownSummary, JournalSummary } from "../src/world/world";
import type { WorldSince } from "../src/world/since";
import type { WorldAgents } from "../src/world/agents";
import type { ChangePreview, LoginChange, MachineLogins, WorldLogins } from "../src/world/logins";
import type { Control, Controlled, Offers } from "../src/world/control";
import type { FileReadPart, FileReadResult } from "../src/world/file-read";
import type { Attachment } from "../src/world/attachments";
import type { Forgetting, Managed } from "../src/world/management";
import type { FileSearchReply, FileSearchRequest } from "../src/world/file-search";
import type { Folders } from "../src/world/focus";
import type { PageView, PageSummary, PageVersion, PageConsent, PageResult } from "../src/world/pages";
import type { Speaker } from "../src/world/events";
import type { Writer, JobOwner, MapView } from "../src/world/world";
import type { Address } from "../src/world/wire";
import type { RunFacts, DiffLine, JobFacts, ProjectedLines, LinePart, LineOpenError } from "../src/world/run-facts";
import type { RenewalStatus, MaintenanceRequest, MaintenanceResponse, VersionRefusal } from "../src/world/renewal";
import type { AccountChange, AccountsReply, TokenPeek } from "../src/world/accounts";
import type { ConnectionState } from "../src/world/device";
import type { Choices } from "../src/world/bridge";
import type { ComposerSnapshot, ComposerMutation, ComposerUpdated } from "../src/world/composer";
import type { DocumentSubscription, DocumentInput, DocumentFrame, DocumentSend } from "../src/world/composer-sync";
import type { MachineLogPage } from "../src/world/machine-log";
import type { MachineStats } from "../src/world/stats";
import type { UsageCallsPage, UsageCallsQuery } from "../src/world/usage-calls";
import type { UsageReport } from "../src/world/usage";
import type { McpReply, ProfileView } from "../src/world/profile";
import type { UpdateState, UpdateCall, CheckFailure, FailureReason, UpdatesViewCall, UpdatesViewReply } from "../src/world/updates";
export const fixture = ${json.trimEnd()} satisfies {
  updates: { device_calls: UpdateCall[]; states: UpdateState[]; checks: CheckFailure[]; reasons: FailureReason[]; calls: UpdatesViewCall[]; replies: UpdatesViewReply[] };
  // An account's own configuration as its pages read and change it.
  profile: { calls: unknown; replies: (({ reply: "profile" } & ProfileView) | { reply: "profile_written"; version: number } | { reply: "mcp"; result: McpReply })[] };
  machine_log: MachineLogPage[];
  composer: { snapshots: ComposerSnapshot[]; mutations: ComposerMutation[]; updated: ComposerUpdated };
  composer_sync: { subscription: DocumentSubscription; inputs: DocumentInput[]; frames: DocumentFrame[]; sends: DocumentSend[] };
  // Accounts as the interface reads and changes them, the device's
  // standing, and what the user chose for a new conversation.
  run_facts: { calls: RunFacts[]; diff: DiffLine[]; updates: JobFacts[]; projected_reads: ProjectedLines[]; parts: LinePart[]; open_errors: LineOpenError[] };
  accounts: { replies: AccountsReply[]; changes: AccountChange[]; connections: ConnectionState[]; choices: Choices[]; peeks: (TokenPeek | null)[] };
  session: {
    events: SessionEvent[];
    watched: Watched[];
    health_cases: { change: SessionChange; health: Health | null }[];
  };
  world: { who: WorldWho[]; nodes: NodesReply[]; towns: TownSummary[]; journals: JournalSummary[] };
  // Only the replies reach the interface; calls and the machines' own
  // telephone go through the shell and the node.
  since: { replies: ({ reply: "since" } & WorldSince)[]; calls: unknown; remote_calls: unknown; remote_replies: unknown };
  agents: { replies: ({ reply: "agents" } & WorldAgents)[]; calls: unknown; remote_calls: unknown; remote_replies: unknown };
  // The agents' logins as each node keeps them, a change's preview and the
  // change itself, step by step.
  logins: {
    calls: unknown;
    replies: (
      | ({ reply: "agent_logins" } & WorldLogins)
      | ({ reply: "machine_agent_logins" } & MachineLogins)
      | ({ reply: "agent_change_preview" } & ChangePreview)
      | ({ reply: "agent_login_changed" } & LoginChange)
    )[];
    remote_calls: unknown;
    remote_replies: unknown;
  };
  control: { controls: Control[]; controlled: Controlled[]; offers: Offers[] };
  // The page sees how a read ended and what was opened; the stream itself
  // is the shell's.
  file_read: {
    calls: unknown;
    replies: { reply: "file_read"; result: FileReadResult }[];
    remote_calls: unknown;
    remote_replies: unknown;
    frames: ({ kind: "file_read"; id: number; part: FileReadPart } | { kind: "reply"; id: number; reply: unknown })[];
    binary_payload: unknown;
  };
  // An upload's answer, and words that carry what was attached.
  attachments: {
    calls: unknown;
    // The upload's answer first; what a Talk with attachments answers is
    // the shell's.
    replies: [{ reply: "uploaded" } & Attachment, ...unknown[]];
    remote_calls: unknown;
    remote_replies: unknown;
    events: SessionEvent[];
    host_text: string;
  };
  // Claude's requests as they come, and the answers its hooks take.
  claude: { events: SessionEvent[]; answers: Control[]; unavailable: Offers; idle: Offers };
  // What the user's management of an agent answers, stage by stage.
  management: { calls: unknown; remote_calls: unknown; replies: ({ reply: "managed"; resident: Address; managed: Managed } | { reply: "forgetting"; resident: Address; how: Forgetting })[]; remote_replies: unknown };
  // What a page's calls answer, and the page as a speaker, a writer and
  // a job's owner.
  pages: {
    calls: unknown;
    replies: (
      | { reply: "page"; view: PageView }
      | { reply: "pages"; town: string; pages: PageSummary[] }
      | { reply: "page_history"; town: string; page: string; versions: PageVersion[] }
      | { reply: "page_consent"; town: string; page: string; consent: PageConsent }
      | { reply: "page_act"; result: PageResult }
    )[];
    remote_calls: unknown;
    remote_replies: unknown;
    speaker: Speaker;
    writer: Writer;
    job_owner: JobOwner;
    peer: unknown;
    map: MapView;
  };
  renewal: { requests: MaintenanceRequest[]; responses: MaintenanceResponse[]; statuses: RenewalStatus[]; refused: VersionRefusal };
  // How a machine says it is doing, for its details page.
  stats: { stats: MachineStats[] };
  // What a login was used for, as its machine's node answers it.
  usage_calls: {
    calls: { call: "agent_calls"; machine: string; agent: string; dir: string; query: UsageCallsQuery }[];
    replies: ({ reply: "agent_calls" } & UsageCallsPage)[];
    remote_calls: unknown; remote_replies: unknown;
  };
  usage: { calls: unknown; replies: ({ reply: "agent_usage" } & UsageReport)[]; remote_calls: unknown; remote_replies: unknown };
  // Finding files for an @ mention: what the interface asks and is answered.
  file_search: {
    calls: { call: "find_files"; machine: string | null; request: FileSearchRequest }[];
    replies: ({ reply: "files_found" } & FileSearchReply)[];
    remote_calls: unknown;
    remote_results: unknown;
  };
  // Walking a machine's folders to point a conversation at some.
  focus: {
    calls: { call: "folders"; machine: string | null; path: string | null }[];
    replies: ({ reply: "folders" } & Folders)[];
    remote_calls: unknown;
    remote_results: unknown;
  };
};
`;
const configFile = ts.readConfigFile(path.join(gui, "tsconfig.json"), ts.sys.readFile);
if (configFile.error) throw new Error(ts.flattenDiagnosticMessageText(configFile.error.messageText, "\n"));
const config = ts.parseJsonConfigFileContent(configFile.config, ts.sys, gui);
const host = ts.createCompilerHost(config.options);
const getSourceFile = host.getSourceFile.bind(host);
host.getSourceFile = (fileName, languageVersion, onError, shouldCreateNewSourceFile) =>
  path.resolve(fileName) === virtualFile
    ? ts.createSourceFile(fileName, source, languageVersion, true)
    : getSourceFile(fileName, languageVersion, onError, shouldCreateNewSourceFile);
const program = ts.createProgram([...config.fileNames, virtualFile], config.options, host);
const diagnostics = [...config.errors, ...ts.getPreEmitDiagnostics(program)];
if (diagnostics.length) {
  process.stderr.write(ts.formatDiagnosticsWithColorAndContext(diagnostics, {
    getCanonicalFileName: (fileName) => fileName,
    getCurrentDirectory: () => gui,
    getNewLine: () => "\n",
  }));
  process.exit(1);
}

// The model and what it imports run without a DOM: bundled into one module.
const bundled = await build({
  entryPoints: [path.join(gui, "src/conversation/model.ts")],
  bundle: true,
  write: false,
  format: "esm",
  platform: "neutral",
  logLevel: "silent",
});
const outputText = bundled.outputFiles[0].text;
const { changeHealth, Fold } = await import(`data:text/javascript;base64,${Buffer.from(outputText).toString("base64")}`);
for (const { change, health } of fixture.session.health_cases) {
  assert.deepEqual(changeHealth(change), health, `SessionChange::health() differs for ${JSON.stringify(change)}`);
}
const conversation = new Fold();
conversation.history(fixture.session.events);
assert.equal(conversation.title, "Wire contract");
assert(conversation.entries.some((entry) => entry.kind === "tool"));
assert(conversation.entries.some((entry) => entry.kind === "uncertain"));
assert.equal(conversation.moving(), false);

// Claude's requests become cards, and the cards answer in the shape its
// hooks take: allowing once, a form filled in.
const asking = new Fold();
asking.history(fixture.claude.events.filter((event) => event.change?.kind !== "resolved"));
const asks = asking.entries.filter((entry) => entry.kind === "ask");
assert.equal(asks.length, 2, "each Claude request is a card");
const cards = await build({
  stdin: {
    contents: `export { readAsk, answerOf } from "./src/conversation/asks"; export { zhComposer } from "./src/app/words";`,
    resolveDir: gui,
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  // The cards draw prose, whose packages resolve as Node's do.
  platform: "node",
  logLevel: "silent",
});
const { readAsk, answerOf, zhComposer } = await import(`data:text/javascript;base64,${Buffer.from(cards.outputFiles[0].text).toString("base64")}`);
/** A card as answerOf reads it: what is typed in its fields. */
const card = ({ fields = {} }) => {
  const find = (selector) => {
    const field = /\.ask-input\[data-field="(\d+)"\]/.exec(selector);
    return field && fields[field[1]] !== undefined ? [{ value: fields[field[1]] }] : [];
  };
  return { querySelectorAll: find, querySelector: (selector) => find(selector)[0] ?? null };
};
const given = [
  [{}, { decision: 0 }],
  [{ fields: { 0: "Hangzhou" } }, { submit: "accept" }],
];
for (const [index, ask] of asks.entries()) {
  const expected = fixture.claude.answers[index];
  assert.deepEqual(ask.id, expected.request, "answers follow their requests");
  const read = readAsk(ask.request, ask.params, zhComposer.asks);
  const answered = answerOf(card(given[index][0]), read, given[index][1], zhComposer.asks);
  assert.deepEqual(answered.answer, expected.answer, `the answer to ${ask.request}`);
}
process.stdout.write(`Wire contract: ${fixture.session.events.length} events, ${fixture.session.health_cases.length} Rust health comparisons, ${fixture.world.who.length} who replies, ${fixture.world.nodes.length} nodes replies, ${fixture.world.towns.length} towns, ${fixture.world.journals.length} journals, ${asks.length} Claude answers passed.\n`);
