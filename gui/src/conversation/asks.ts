// What a session waits on the user for: a command or a change to approve, a
// form or a link from one of its tools. It asks the user in words through
// `message_user`, which waits for nothing; the hosts' own asking tools are
// off in the World's sessions. The host asks; the
// World passes the request on as it came (`SessionChange::Request`) and
// carries the user's answer back in the host's own shape. A card is drawn
// where the request came, answered in place, and folds into a line once it
// is settled. Requests the World cannot answer for the user are said to be
// waiting, never answered by guess.

import type { JsonValue } from "../world/wire";
import { forPeople, type AskState } from "./ask-state";
import { glyph } from "./glyphs";
import { escape, plain, render } from "./prose";

export interface AskWords {
  waiting: string;
  command(machine: string): string;
  stdin(machine: string): string;
  network(host: string, protocol: string): string;
  files: string;
  filesNamed(count: number): string;
  grantRoot(root: string): string;
  reason(why: string): string;
  inDir(cwd: string): string;
  accept: string;
  acceptForSession: string;
  acceptPrefix(prefix: string): string;
  networkRule(allow: boolean, host: string): string;
  decline: string;
  cancel: string;
  fill: string;
  form(server: string): string;
  link(server: string): string;
  openLink: string;
  refuse: string;
  dismiss: string;
  required: string;
  requiredOf(name: string): string;
  formUnreadable: string;
  sending: string;
  failed(why: string): string;
  cannot(request: string): string;
  permissions: string;
  verification: string;
  /** A Claude tool other than a command or a file change. */
  tool(name: string): string;
  /** Claude's plan, waiting for leave to carry it out. */
  plan: string;
  planAt(path: string): string;
  planGo: string;
  /** Carrying it out while keeping a permission Claude proposes. */
  planGoWith(kept: string): string;
  planKeep: string;
  /** The box for what the plan should change, sent with keeping it planning. */
  planNote: string;
  /** A permission Claude proposes keeping, said with where it is kept. */
  rule(allow: "allow" | "deny" | "ask", rules: string, where: string): string;
  unrule(rules: string, where: string): string;
  mode(mode: string, where: string): string;
  directories(add: boolean, directories: string, where: string): string;
  kept: { session: string; localSettings: string; projectSettings: string; userSettings: string; cliArg: string };
  answered(said: string): string;
  gone: string;
  settledBy: { accept: string; session: string; rule: string; decline: string; cancel: string; answered: string; skipped: string; submitted: string; refused: string; dismissed: string; opened: string; planGo: string; planKeep: string };
}

/** One answer a card offers: the host's own value, how it is said, and
 *  how it reads once given. */
export interface Decision {
  value: JsonValue;
  label: string;
  settled: string;
  tone: "yes" | "no";
}

export type Field =
  | { name: string; title: string; description: string; required: boolean; kind: "text"; input: string; min?: number; max?: number; value: string }
  | { name: string; title: string; description: string; required: boolean; kind: "number"; integer: boolean; min?: number; max?: number; value: string }
  | { name: string; title: string; description: string; required: boolean; kind: "boolean"; value: boolean }
  | { name: string; title: string; description: string; required: boolean; kind: "one"; options: { value: string; label: string }[]; value: string | null }
  | { name: string; title: string; description: string; required: boolean; kind: "many"; options: { value: string; label: string }[]; value: string[] };

/** Whose shape an answer takes: Codex's app-server, or Claude Code's hooks
 *  (`crates/fastctx-types/examples/gui_wire_fixtures/claude.rs`). */
type Host = "codex" | "claude";

export type Ask =
  | { kind: "command"; command: string | null; cwd: string | null; reason: string | null; stdin: boolean; network: { host: string; protocol: string } | null; decisions: Decision[] }
  | { kind: "files"; item: string | null; reason: string | null; grantRoot: string | null; paths: string[]; decisions: Decision[] }
  /** A Claude tool asking leave to run, other than a command or a change. */
  | { kind: "tool"; tool: string; input: string; decisions: Decision[] }
  /** Claude's plan, handed over for leave to leave plan mode; `check`
   *  when asked through the mod's tool check, which answers allow or deny
   *  with a reason. */
  | { kind: "plan"; plan: string; path: string | null; decisions: Decision[]; check?: boolean }
  | { kind: "form"; server: string; message: string; fields: Field[] | null; host: Host }
  | { kind: "link"; server: string; message: string; url: string; host: Host }
  | { kind: "cannot"; what: string; detail: string | null }
  /** Not for a person to answer: the node itself refuses these. */
  | { kind: "hidden" };


/** Reads a request as the card shows it. */
export function readAsk(request: string, params: JsonValue, words: AskWords): Ask {
  if (!forPeople(request)) return { kind: "hidden" };
  const p = object(params);
  switch (request) {
    case "item/commandExecution/requestApproval": {
      const network = object(p.networkApprovalContext ?? null);
      return {
        kind: "command",
        command: text(p.command),
        cwd: text(p.cwd),
        reason: text(p.reason),
        stdin: p.kind === "writeStdin",
        network: typeof network.host === "string" ? { host: network.host, protocol: text(network.protocol) ?? "" } : null,
        decisions: commandDecisions(p, words),
      };
    }
    case "item/fileChange/requestApproval":
      return {
        kind: "files",
        item: text(p.itemId),
        reason: text(p.reason),
        grantRoot: text(p.grantRoot),
        paths: [],
        decisions: (["accept", "acceptForSession", "decline", "cancel"] as const).map((value) => simple(value, words)),
      };
    case "execCommandApproval": {
      const argv = Array.isArray(p.command) ? p.command.filter((part): part is string => typeof part === "string") : [];
      return {
        kind: "command",
        command: argv.join(" ") || null,
        cwd: text(p.cwd),
        reason: text(p.reason),
        stdin: false,
        network: null,
        decisions: legacyDecisions(words),
      };
    }
    case "applyPatchApproval":
      return {
        kind: "files",
        item: null,
        reason: text(p.reason),
        grantRoot: text(p.grantRoot),
        paths: Object.keys(object(p.fileChanges ?? null)),
        decisions: legacyDecisions(words),
      };
    case "classic.PermissionRequest":
      return claudePermission(p, words);
    case "tool.check":
      return claudeCheck(p, words);
    case "Elicitation": {
      const server = text(p.mcp_server_name) ?? "";
      const message = text(p.message) ?? "";
      if (p.mode === "url") return { kind: "link", server, message, url: text(p.url) ?? "", host: "claude" };
      return { kind: "form", server, message, fields: fieldsOf(p.requested_schema ?? null), host: "claude" };
    }
    case "mcpServer/elicitation/request": {
      const server = text(p.serverName) ?? "";
      const message = text(p.message) ?? "";
      switch (p.mode) {
        case "form":
        case "openai/form":
        case "openaiForm":
          return { kind: "form", server, message, fields: fieldsOf(p.requestedSchema ?? null), host: "codex" };
        case "url":
          return { kind: "link", server, message, url: text(p.url) ?? "", host: "codex" };
        case "openai/userVerification":
          return { kind: "cannot", what: words.verification, detail: text(p.description) ?? text(p.title) };
        default:
          return { kind: "cannot", what: words.cannot(request), detail: null };
      }
    }
    case "item/permissions/requestApproval":
      return { kind: "cannot", what: words.permissions, detail: text(p.reason) };
    default:
      return { kind: "cannot", what: words.cannot(request), detail: null };
  }
}

/** Claude asking leave for a tool: a command is shown as one, a change by
 *  the file it touches, anything else by its tool and what it was given.
 *  Beside allowing once, the permissions Claude itself proposes to keep are
 *  offered, each said with where it would be kept: some are written into
 *  Claude's settings and last beyond this conversation. */
function claudePermission(p: Record<string, JsonValue>, words: AskWords): Ask {
  const tool = text(p.tool_name) ?? "";
  const input = object(p.tool_input ?? null);
  const suggestions = Array.isArray(p.permission_suggestions) ? p.permission_suggestions : [];
  if (tool === "ExitPlanMode") return claudePlan(input, suggestions, words);
  const decisions: Decision[] = [{ value: { behavior: "allow" }, label: words.accept, settled: words.settledBy.accept, tone: "yes" }];
  for (const suggestion of suggestions) {
    const label = keptAs(object(suggestion), words);
    if (label) decisions.push({ value: { behavior: "allow", updatedPermissions: [suggestion] }, label, settled: words.settledBy.rule, tone: "yes" });
  }
  decisions.push(
    { value: { behavior: "deny" }, label: words.decline, settled: words.settledBy.decline, tone: "no" },
    { value: { behavior: "deny", interrupt: true }, label: words.cancel, settled: words.settledBy.cancel, tone: "no" },
  );
  if (tool === "Bash") {
    return { kind: "command", command: text(input.command), cwd: text(p.cwd), reason: text(input.description), stdin: false, network: null, decisions };
  }
  const path = text(input.file_path) ?? text(input.notebook_path);
  if (path && ["Edit", "MultiEdit", "Write", "NotebookEdit"].includes(tool)) {
    return { kind: "files", item: null, reason: null, grantRoot: null, paths: [path], decisions };
  }
  return { kind: "tool", tool, input: JSON.stringify(input, null, 2), decisions };
}

/** Claude asking leave for a tool through the mod's tool check: allow or
 *  deny, with the reason Claude Code's own check gave shown above. It
 *  offers no rule to keep, as that check answers only for this call. */
function claudeCheck(p: Record<string, JsonValue>, words: AskWords): Ask {
  const tool = text(p.tool) ?? "";
  const input = object(p.input ?? null);
  const verdict = object(p.verdict ?? null);
  const why = text(verdict.reason);
  if (tool === "ExitPlanMode") {
    return {
      kind: "plan",
      plan: text(input.plan) ?? "",
      path: text(input.planFilePath),
      check: true,
      decisions: [
        { value: "allow", label: words.planGo, settled: words.settledBy.planGo, tone: "yes" },
        { value: "deny", label: words.planKeep, settled: words.settledBy.planKeep, tone: "no" },
      ],
    };
  }
  const decisions: Decision[] = [
    { value: "allow", label: words.accept, settled: words.settledBy.accept, tone: "yes" },
    { value: "deny", label: words.decline, settled: words.settledBy.decline, tone: "no" },
  ];
  if (tool === "Bash") {
    return { kind: "command", command: text(input.command), cwd: null, reason: text(input.description) ?? why, stdin: false, network: null, decisions };
  }
  const path = text(input.file_path) ?? text(input.notebook_path);
  if (path && ["Edit", "MultiEdit", "Write", "NotebookEdit"].includes(tool)) {
    return { kind: "files", item: null, reason: why, grantRoot: null, paths: [path], decisions };
  }
  return { kind: "tool", tool, input: JSON.stringify(input, null, 2), decisions };
}

/** Claude's plan, asking leave to leave plan mode. Claude Code puts the
 *  plan and the file it wrote it to into the tool's input; allowing hands
 *  that input back unchanged, as tools that wait on the user take it, and
 *  the modes Claude proposes for afterwards are offered as it proposes
 *  them. Keeping it planning carries the user's note, when there is one. */
function claudePlan(input: Record<string, JsonValue>, suggestions: JsonValue[], words: AskWords): Ask {
  const allow = { behavior: "allow", updatedInput: input };
  const decisions: Decision[] = [{ value: allow, label: words.planGo, settled: words.settledBy.planGo, tone: "yes" }];
  for (const suggestion of suggestions) {
    const kept = keptAs(object(suggestion), words);
    if (kept) decisions.push({ value: { ...allow, updatedPermissions: [suggestion] }, label: words.planGoWith(kept), settled: words.settledBy.planGo, tone: "yes" });
  }
  decisions.push(
    { value: { behavior: "deny" }, label: words.planKeep, settled: words.settledBy.planKeep, tone: "no" },
    { value: { behavior: "deny", interrupt: true }, label: words.cancel, settled: words.settledBy.cancel, tone: "no" },
  );
  return { kind: "plan", plan: text(input.plan) ?? "", path: text(input.planFilePath), decisions };
}

/** A permission Claude proposes keeping, as the user reads it; null for a
 *  kind this card cannot say plainly, which is then not offered. */
function keptAs(s: Record<string, JsonValue>, words: AskWords): string | null {
  const where = words.kept[text(s.destination) as keyof AskWords["kept"]];
  if (!where) return null;
  const rules = (Array.isArray(s.rules) ? s.rules.map(object) : [])
    .map((rule) => `${text(rule.toolName) ?? ""}${text(rule.ruleContent) ? `(${text(rule.ruleContent)})` : ""}`)
    .join(", ");
  const directories = (Array.isArray(s.directories) ? s.directories : []).filter((d): d is string => typeof d === "string").join(", ");
  switch (s.type) {
    case "addRules":
    case "replaceRules":
      return s.behavior === "allow" || s.behavior === "deny" || s.behavior === "ask" ? words.rule(s.behavior, rules, where) : null;
    case "removeRules":
      return words.unrule(rules, where);
    case "setMode":
      return text(s.mode) ? words.mode(text(s.mode)!, where) : null;
    case "addDirectories":
    case "removeDirectories":
      return directories ? words.directories(s.type === "addDirectories", directories, where) : null;
    default:
      return null;
  }
}

function simple(value: "accept" | "acceptForSession" | "decline" | "cancel", words: AskWords): Decision {
  switch (value) {
    case "accept":
      return { value, label: words.accept, settled: words.settledBy.accept, tone: "yes" };
    case "acceptForSession":
      return { value, label: words.acceptForSession, settled: words.settledBy.session, tone: "yes" };
    case "decline":
      return { value, label: words.decline, settled: words.settledBy.decline, tone: "no" };
    case "cancel":
      return { value, label: words.cancel, settled: words.settledBy.cancel, tone: "no" };
  }
}

/** The decisions a command approval offers: those the host lists, in its
 *  order; where it lists none, those it accepts for that kind of request
 *  (the same rule the node checks answers by). */
function commandDecisions(p: Record<string, JsonValue>, words: AskWords): Decision[] {
  const listed: JsonValue[] = Array.isArray(p.availableDecisions) ? p.availableDecisions : fallbackDecisions(p);
  return listed.flatMap((value): Decision[] => {
    if (value === "accept" || value === "acceptForSession" || value === "decline" || value === "cancel") return [simple(value, words)];
    const shape = object(value);
    const execpolicy = object(shape.acceptWithExecpolicyAmendment ?? null).execpolicy_amendment;
    if (Array.isArray(execpolicy)) {
      const prefix = execpolicy.filter((part): part is string => typeof part === "string").join(" ");
      return [{ value, label: words.acceptPrefix(prefix), settled: words.settledBy.rule, tone: "yes" }];
    }
    const network = object(object(shape.applyNetworkPolicyAmendment ?? null).network_policy_amendment ?? null);
    if (typeof network.host === "string") {
      const allow = network.action === "allow";
      return [{ value, label: words.networkRule(allow, network.host), settled: words.settledBy.rule, tone: allow ? "yes" : "no" }];
    }
    return [];
  });
}

function fallbackDecisions(p: Record<string, JsonValue>): JsonValue[] {
  const decisions: JsonValue[] = ["accept"];
  if (p.networkApprovalContext !== undefined && p.networkApprovalContext !== null) {
    decisions.push("acceptForSession");
    const amendments = Array.isArray(p.proposedNetworkPolicyAmendments) ? p.proposedNetworkPolicyAmendments : [];
    const allow = amendments.find((amendment) => object(amendment).action === "allow");
    if (allow !== undefined) decisions.push({ applyNetworkPolicyAmendment: { network_policy_amendment: allow } });
  } else if ((p.additionalPermissions ?? null) === null && p.proposedExecpolicyAmendment !== undefined && p.proposedExecpolicyAmendment !== null) {
    decisions.push({ acceptWithExecpolicyAmendment: { execpolicy_amendment: p.proposedExecpolicyAmendment } });
  }
  decisions.push("cancel");
  return decisions;
}

function legacyDecisions(words: AskWords): Decision[] {
  return [
    { value: "approved", label: words.accept, settled: words.settledBy.accept, tone: "yes" },
    { value: "approved_for_session", label: words.acceptForSession, settled: words.settledBy.session, tone: "yes" },
    { value: "abort", label: words.cancel, settled: words.settledBy.cancel, tone: "no" },
  ];
}

/** The fields of a form, when its schema is one a card can draw: an object
 *  of flat fields, as MCP elicitation allows. */
function fieldsOf(schema: JsonValue): Field[] | null {
  const s = object(schema);
  if (s.type !== "object") return null;
  const properties = object(s.properties ?? null);
  const required = new Set(Array.isArray(s.required) ? s.required.filter((name): name is string => typeof name === "string") : []);
  const fields: Field[] = [];
  for (const [name, raw] of Object.entries(properties)) {
    const f = object(raw);
    const base = { name, title: text(f.title) ?? name, description: text(f.description) ?? "", required: required.has(name) };
    const options = choicesOf(f);
    if (f.type === "array") {
      const items = object(f.items ?? null);
      const many = choicesOf(items);
      if (!many) return null;
      const chosen = Array.isArray(f.default) ? f.default.filter((v): v is string => typeof v === "string") : [];
      fields.push({ ...base, kind: "many", options: many, value: chosen });
    } else if (options) {
      fields.push({ ...base, kind: "one", options, value: text(f.default) });
    } else if (f.type === "string") {
      const input = f.format === "email" ? "email" : f.format === "uri" ? "url" : f.format === "date" ? "date" : f.format === "date-time" ? "datetime-local" : "text";
      fields.push({ ...base, kind: "text", input, ...bounds(f.minLength, f.maxLength), value: text(f.default) ?? "" });
    } else if (f.type === "number" || f.type === "integer") {
      fields.push({ ...base, kind: "number", integer: f.type === "integer", ...bounds(f.minimum, f.maximum), value: typeof f.default === "number" ? String(f.default) : "" });
    } else if (f.type === "boolean") {
      fields.push({ ...base, kind: "boolean", value: f.default === true });
    } else {
      return null;
    }
  }
  return fields;
}

function choicesOf(f: Record<string, JsonValue>): { value: string; label: string }[] | null {
  const titled = Array.isArray(f.oneOf) ? f.oneOf : Array.isArray(f.anyOf) ? f.anyOf : null;
  if (titled) {
    return titled.map(object).map((option) => ({ value: text(option.const) ?? "", label: text(option.title) ?? text(option.const) ?? "" }));
  }
  if (Array.isArray(f.enum)) {
    const names = Array.isArray(f.enumNames) ? f.enumNames : [];
    return f.enum.map((value, index) => ({ value: String(value), label: text(names[index] ?? null) ?? String(value) }));
  }
  return null;
}

function bounds(min: JsonValue | undefined, max: JsonValue | undefined): { min?: number; max?: number } {
  return { ...(typeof min === "number" ? { min } : {}), ...(typeof max === "number" ? { max } : {}) };
}

function object(value: JsonValue): Record<string, JsonValue> {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, JsonValue>) : {};
}

function text(value: JsonValue | undefined): string | null {
  return typeof value === "string" && value !== "" ? value : null;
}


/** The card, drawn: its question while it waits, one quiet line once
 *  settled. `machine` is where the agent lives; `changes` lists the files a
 *  change approval is about, read from the conversation. */
export function drawAsk(key: string, ask: Ask, state: AskState, machine: string, words: AskWords): string {
  if (ask.kind === "hidden") return "";
  if (state.state === "answered" || state.state === "gone") {
    const said = state.state === "answered" ? words.answered(state.said) : words.gone;
    return `<div class="ask-settled${state.state === "gone" ? " faint" : ""}">${glyph("check", 16)}<span>${escape(said)}</span></div>`;
  }
  const head = (title: string) => `<div class="ask-head"><span class="ask-waiting">${escape(words.waiting)}</span><span class="ask-title">${escape(title)}</span></div>`;
  const reason = (why: string | null) => (why ? `<p class="ask-reason">${escape(words.reason(why))}</p>` : "");
  // What is asked above, the answer below, one stroke of the pen between.
  const rule = `<div class="ask-rule" aria-hidden="true"></div>`;
  const buttons = (decisions: Decision[]) =>
    `<div class="ask-actions">${decisions
      .map((d, index) => `<button type="button" class="ask-button ${index === 0 ? "ask-primary" : `ask-${d.tone}`}" data-ask="${escape(key)}" data-decision="${index}">${escape(d.label)}</button>`)
      .join("")}</div>`;
  let body: string;
  switch (ask.kind) {
    case "command": {
      const title = ask.network ? words.network(ask.network.host, ask.network.protocol) : ask.stdin ? words.stdin(machine) : words.command(machine);
      const command = ask.command ? `<pre class="ask-command"><code>${escape(ask.command)}</code></pre>` : "";
      const where = ask.cwd ? `<p class="ask-where">${escape(words.inDir(ask.cwd))}</p>` : "";
      body = `${head(title)}${command}${where}${reason(ask.reason)}${rule}${buttons(ask.decisions)}`;
      break;
    }
    case "files": {
      const paths = ask.paths.length
        ? `<ul class="ask-paths">${ask.paths.map((path) => `<li title="${escape(path)}">${escape(path)}</li>`).join("")}</ul>`
        : "";
      const root = ask.grantRoot ? `<p class="ask-reason">${escape(words.grantRoot(ask.grantRoot))}</p>` : "";
      body = `${head(ask.paths.length ? words.filesNamed(ask.paths.length) : words.files)}${paths}${reason(ask.reason)}${root}${rule}${buttons(ask.decisions)}`;
      break;
    }
    case "tool": {
      const input = ask.input !== "{}" ? `<pre class="ask-command ask-tool-input"><code>${escape(ask.input)}</code></pre>` : "";
      body = `${head(words.tool(ask.tool))}${input}${rule}${buttons(ask.decisions)}`;
      break;
    }
    case "plan": {
      const plan = ask.plan ? `<div class="ask-plan prose">${render(ask.plan)}</div>` : "";
      const where = ask.path ? `<p class="ask-where">${escape(words.planAt(ask.path))}</p>` : "";
      const note = `<textarea class="ask-input ask-plan-note" rows="2" aria-label="${escape(words.planNote)}" placeholder="${escape(words.planNote)}"></textarea>`;
      body = `${head(words.plan)}${plan}${where}${rule}${note}${buttons(ask.decisions)}`;
      break;
    }
    case "form": {
      const message = ask.message ? `<div class="ask-message">${plain(ask.message)}</div>` : "";
      const fields = ask.fields ? `<div class="ask-fields">${ask.fields.map((field, index) => drawField(field, index, words)).join("")}</div>` : `<p class="ask-reason">${escape(words.formUnreadable)}</p>`;
      const submit = ask.fields ? `<button type="button" class="ask-button ask-primary" data-ask="${escape(key)}" data-submit="accept">${escape(words.fill)}</button>` : "";
      body = `${head(words.form(ask.server))}${message}${rule}${fields}<div class="ask-actions">${submit}<button type="button" class="ask-button ask-no" data-ask="${escape(key)}" data-submit="decline">${escape(words.refuse)}</button><button type="button" class="ask-button ask-no" data-ask="${escape(key)}" data-submit="cancel">${escape(words.dismiss)}</button></div>`;
      break;
    }
    case "link": {
      const message = ask.message ? `<div class="ask-message">${plain(ask.message)}</div>` : "";
      body = `${head(words.link(ask.server))}${message}<p class="ask-url">${escape(ask.url)}</p>${rule}<div class="ask-actions"><button type="button" class="ask-button ask-primary" data-ask="${escape(key)}" data-submit="open">${escape(words.openLink)}</button><button type="button" class="ask-button ask-no" data-ask="${escape(key)}" data-submit="decline">${escape(words.refuse)}</button><button type="button" class="ask-button ask-no" data-ask="${escape(key)}" data-submit="cancel">${escape(words.dismiss)}</button></div>`;
      break;
    }
    case "cannot":
      body = `${head(ask.what)}${ask.detail ? `<p class="ask-reason">${escape(ask.detail)}</p>` : ""}`;
      break;
  }
  return `<div class="ask-card" role="group" aria-label="${escape(words.waiting)}" data-card="${escape(key)}">${body}<p class="ask-status" aria-live="polite"></p></div>`;
}

function drawField(field: Field, index: number, words: AskWords): string {
  const label = `<span class="ask-field-title">${escape(field.title)}${field.required ? `<span class="ask-required" title="${escape(words.required)}">*</span>` : ""}</span>${field.description ? `<span class="ask-field-what">${escape(field.description)}</span>` : ""}`;
  switch (field.kind) {
    case "text":
      return `<label class="ask-field">${label}<input class="ask-input" data-field="${index}" type="${field.input}" value="${escape(field.value)}"${field.min !== undefined ? ` minlength="${field.min}"` : ""}${field.max !== undefined ? ` maxlength="${field.max}"` : ""}${field.required ? " required" : ""}></label>`;
    case "number":
      return `<label class="ask-field">${label}<input class="ask-input" data-field="${index}" type="number" inputmode="${field.integer ? "numeric" : "decimal"}" step="${field.integer ? "1" : "any"}" value="${escape(field.value)}"${field.min !== undefined ? ` min="${field.min}"` : ""}${field.max !== undefined ? ` max="${field.max}"` : ""}${field.required ? " required" : ""}></label>`;
    case "boolean":
      return `<label class="ask-field ask-field-check"><input class="ask-check" data-field="${index}" type="checkbox"${field.value ? " checked" : ""}>${label}</label>`;
    case "one":
    case "many":
      return `<div class="ask-field" role="group" aria-label="${escape(field.title)}">${label}<div class="ask-options">${field.options
        .map((option, n) => {
          const on = field.kind === "one" ? field.value === option.value : field.value.includes(option.value);
          return `<button type="button" class="ask-option${on ? " chosen" : ""}" data-field="${index}" data-option="${n}"${field.kind === "many" ? ' data-many="1"' : ""} aria-pressed="${on}"><span class="ask-option-label">${escape(option.label)}</span></button>`;
        })
        .join("")}</div></div>`;
  }
}

/** Reads what the user gave in a card as the host's answer; a reason when
 *  it cannot be sent as it stands. */
export function answerOf(card: HTMLElement, ask: Ask, given: { decision?: number; submit?: string }, words: AskWords): { answer: JsonValue; said: string } | { missing: string } {
  switch (ask.kind) {
    case "command":
    case "files":
    case "tool": {
      const decision = ask.decisions[given.decision ?? -1];
      if (!decision) return { missing: words.required };
      return { answer: { decision: decision.value }, said: decision.settled };
    }
    case "plan": {
      const decision = ask.decisions[given.decision ?? -1];
      if (!decision) return { missing: words.required };
      const note = card.querySelector<HTMLTextAreaElement>(".ask-plan-note")?.value.trim() ?? "";
      // The tool check carries what the plan should change as its reason.
      if (ask.check) {
        return { answer: decision.value === "deny" && note ? { decision: "deny", reason: note } : { decision: decision.value }, said: decision.settled };
      }
      const value = object(decision.value);
      // What the user wants changed goes back with keeping it planning.
      const answer = value.behavior === "deny" && value.interrupt === undefined && note ? { ...value, message: note } : value;
      return { answer: { decision: answer }, said: decision.settled };
    }
    case "form": {
      if (given.submit !== "accept") return refusal(given.submit, words, ask.host);
      if (!ask.fields) return { missing: words.required };
      const content: Record<string, JsonValue> = {};
      for (const [index, field] of ask.fields.entries()) {
        const value = fieldValue(card, field, index);
        if (value === null) {
          if (field.required) return { missing: words.requiredOf(field.title) };
          continue;
        }
        content[field.name] = value;
      }
      return {
        answer: ask.host === "claude" ? { action: "accept", content } : { action: "accept", content, _meta: null },
        said: words.settledBy.submitted,
      };
    }
    case "link":
      if (given.submit !== "open") return refusal(given.submit, words, ask.host);
      return {
        answer: ask.host === "claude" ? { action: "accept" } : { action: "accept", content: null, _meta: null },
        said: words.settledBy.opened,
      };
    case "cannot":
    case "hidden":
      return { missing: words.required };
  }
}

function refusal(submit: string | undefined, words: AskWords, host: Host): { answer: JsonValue; said: string } {
  const action = submit === "cancel" ? "cancel" : "decline";
  return {
    answer: host === "claude" ? { action } : { action, content: null, _meta: null },
    said: action === "cancel" ? words.settledBy.dismissed : words.settledBy.refused,
  };
}

function fieldValue(card: HTMLElement, field: Field, index: number): JsonValue | null {
  switch (field.kind) {
    case "text": {
      const raw = card.querySelector<HTMLInputElement>(`.ask-input[data-field="${index}"]`)?.value ?? "";
      if (!raw) return null;
      // A date and time is sent as RFC 3339, which the browser's local
      // picker does not write.
      if (field.input === "datetime-local") {
        const at = new Date(raw);
        return Number.isNaN(at.getTime()) ? raw : at.toISOString();
      }
      return raw;
    }
    case "number": {
      const raw = card.querySelector<HTMLInputElement>(`.ask-input[data-field="${index}"]`)?.value ?? "";
      return raw === "" ? null : Number(raw);
    }
    case "boolean":
      return card.querySelector<HTMLInputElement>(`.ask-check[data-field="${index}"]`)?.checked ?? false;
    case "one": {
      const chosen = card.querySelector<HTMLElement>(`.ask-option[data-field="${index}"][aria-pressed="true"]`);
      return chosen ? field.options[Number(chosen.dataset.option)]!.value : null;
    }
    case "many": {
      const chosen = [...card.querySelectorAll<HTMLElement>(`.ask-option[data-field="${index}"][aria-pressed="true"]`)];
      return chosen.length ? chosen.map((button) => field.options[Number(button.dataset.option)]!.value) : field.required ? null : [];
    }
  }
}
