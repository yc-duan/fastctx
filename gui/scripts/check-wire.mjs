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
  const generated = execFileSync("cargo", ["run", "--locked", "--quiet", "-p", "fastctx-types", "--example", "gui_wire_fixtures"], {
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
import type { Control, Controlled, Offers } from "../src/world/control";
import type { FileReadPart, FileReadResult } from "../src/world/file-read";
import type { Attachment } from "../src/world/attachments";
import type { RenewalStatus, MaintenanceRequest, MaintenanceResponse, VersionRefusal } from "../src/world/renewal";
export const fixture = ${json.trimEnd()} satisfies {
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
  renewal: { requests: MaintenanceRequest[]; responses: MaintenanceResponse[]; statuses: RenewalStatus[]; refused: VersionRefusal };
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
const { changeHealth, Conversation } = await import(`data:text/javascript;base64,${Buffer.from(outputText).toString("base64")}`);
for (const { change, health } of fixture.session.health_cases) {
  assert.deepEqual(changeHealth(change), health, `SessionChange::health() differs for ${JSON.stringify(change)}`);
}
const conversation = new Conversation();
conversation.history(fixture.session.events);
assert.equal(conversation.title, "Wire contract");
assert(conversation.entries.some((entry) => entry.kind === "tool"));
assert(conversation.entries.some((entry) => entry.kind === "uncertain"));
assert.equal(conversation.moving(), false);

// Claude's requests become cards, and the cards answer in the shape its
// hooks take: allowing once, the questions chosen, a form filled in.
const asking = new Conversation();
asking.history(fixture.claude.events.filter((event) => event.change?.kind !== "resolved"));
const asks = asking.entries.filter((entry) => entry.kind === "ask");
assert.equal(asks.length, 3, "each Claude request is a card");
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
/** A card as answerOf reads it: which options are pressed, what is typed. */
const card = ({ pressed = [], fields = {} }) => {
  const find = (selector) => {
    const option = /\.ask-option\[data-question="(\d+)"\]\[aria-pressed="true"\]/.exec(selector);
    if (option) return pressed.filter(([q]) => q === Number(option[1])).map(([, n]) => ({ dataset: { option: String(n) } }));
    const field = /\.ask-input\[data-field="(\d+)"\]/.exec(selector);
    return field && fields[field[1]] !== undefined ? [{ value: fields[field[1]] }] : [];
  };
  return { querySelectorAll: find, querySelector: (selector) => find(selector)[0] ?? null };
};
const given = [
  [{}, { decision: 0 }],
  [{ pressed: [[0, 0], [0, 1]] }, { submit: "answer" }],
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
