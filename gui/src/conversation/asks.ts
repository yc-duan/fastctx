// What a session waits on the user for: a command or a change to approve, a
// question, a form or a link from one of its tools. The host asks; the
// World passes the request on as it came (`SessionChange::Request`) and
// carries the user's answer back in the host's own shape. A card is drawn
// where the request came, answered in place, and folds into a line once it
// is settled. Requests the World cannot answer for the user are said to be
// waiting, never answered by guess.

import type { JsonValue } from "../world/wire";
import { glyph } from "./glyphs";
import { escape, plain } from "./prose";

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
  question: string;
  questions(count: number): string;
  otherAnswer: string;
  secret: string;
  submit: string;
  fill: string;
  skip: string;
  form(server: string): string;
  link(server: string): string;
  openLink: string;
  refuse: string;
  dismiss: string;
  required: string;
  formUnreadable: string;
  sending: string;
  failed(why: string): string;
  cannot(request: string): string;
  permissions: string;
  verification: string;
  answered(said: string): string;
  gone: string;
  settledBy: { accept: string; session: string; rule: string; decline: string; cancel: string; answered: string; skipped: string; submitted: string; refused: string; dismissed: string; opened: string };
}

/** One answer a card offers: the host's own value, how it is said, and
 *  how it reads once given. */
export interface Decision {
  value: JsonValue;
  label: string;
  settled: string;
  tone: "yes" | "no";
}

export interface Question {
  id: string;
  header: string;
  question: string;
  other: boolean;
  secret: boolean;
  options: { label: string; description: string }[];
}

export type Field =
  | { name: string; title: string; description: string; required: boolean; kind: "text"; input: string; min?: number; max?: number; value: string }
  | { name: string; title: string; description: string; required: boolean; kind: "number"; integer: boolean; min?: number; max?: number; value: string }
  | { name: string; title: string; description: string; required: boolean; kind: "boolean"; value: boolean }
  | { name: string; title: string; description: string; required: boolean; kind: "one"; options: { value: string; label: string }[]; value: string | null }
  | { name: string; title: string; description: string; required: boolean; kind: "many"; options: { value: string; label: string }[]; value: string[] };

export type Ask =
  | { kind: "command"; command: string | null; cwd: string | null; reason: string | null; stdin: boolean; network: { host: string; protocol: string } | null; decisions: Decision[] }
  | { kind: "files"; item: string | null; reason: string | null; grantRoot: string | null; paths: string[]; decisions: Decision[] }
  | { kind: "questions"; questions: Question[] }
  | { kind: "form"; server: string; message: string; fields: Field[] | null }
  | { kind: "link"; server: string; message: string; url: string }
  | { kind: "cannot"; what: string; detail: string | null }
  /** Not for a person to answer: the node itself refuses these. */
  | { kind: "hidden" };

/** Requests the node answers itself, never a person. */
const machineOnly = new Set(["account/chatgptAuthTokens/refresh", "attestation/generate", "item/tool/call", "currentTime/read"]);

/** Whether a request is one a person answers. */
export function forPeople(request: string): boolean {
  return !machineOnly.has(request);
}

/** Reads a request as the card shows it. */
export function readAsk(request: string, params: JsonValue, words: AskWords): Ask {
  if (machineOnly.has(request)) return { kind: "hidden" };
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
    case "item/tool/requestUserInput": {
      const questions = Array.isArray(p.questions) ? p.questions.map(object) : [];
      return {
        kind: "questions",
        questions: questions.map((q) => ({
          id: text(q.id) ?? "",
          header: text(q.header) ?? "",
          question: text(q.question) ?? "",
          other: q.isOther === true,
          secret: q.isSecret === true,
          options: Array.isArray(q.options)
            ? q.options.map(object).map((o) => ({ label: text(o.label) ?? "", description: text(o.description) ?? "" }))
            : [],
        })),
      };
    }
    case "mcpServer/elicitation/request": {
      const server = text(p.serverName) ?? "";
      const message = text(p.message) ?? "";
      switch (p.mode) {
        case "form":
        case "openai/form":
        case "openaiForm":
          return { kind: "form", server, message, fields: fieldsOf(p.requestedSchema ?? null) };
        case "url":
          return { kind: "link", server, message, url: text(p.url) ?? "" };
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

/** A request's id as a key: the same id however its keys were ordered. */
export function askKey(id: JsonValue): string {
  return JSON.stringify(sorted(id));
}

function sorted(value: JsonValue): JsonValue {
  if (Array.isArray(value)) return value.map(sorted);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(Object.keys(value).sort().map((key) => [key, sorted((value as Record<string, JsonValue>)[key]!)]));
  }
  return value;
}

/** How a request stands. While an answer is on its way the card says so
 *  itself; the request is settled only once the host has it. */
export type AskState = { state: "waiting" } | { state: "answered"; said: string } | { state: "gone" };

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
  const buttons = (decisions: Decision[]) =>
    `<div class="ask-actions">${decisions
      .map((d, index) => `<button type="button" class="ask-button ${index === 0 ? "ask-primary" : `ask-${d.tone}`}" data-ask="${key}" data-decision="${index}">${escape(d.label)}</button>`)
      .join("")}</div>`;
  let body: string;
  switch (ask.kind) {
    case "command": {
      const title = ask.network ? words.network(ask.network.host, ask.network.protocol) : ask.stdin ? words.stdin(machine) : words.command(machine);
      const command = ask.command ? `<pre class="ask-command"><code>${escape(ask.command)}</code></pre>` : "";
      const where = ask.cwd ? `<p class="ask-where">${escape(words.inDir(ask.cwd))}</p>` : "";
      body = `${head(title)}${command}${where}${reason(ask.reason)}${buttons(ask.decisions)}`;
      break;
    }
    case "files": {
      const paths = ask.paths.length
        ? `<ul class="ask-paths">${ask.paths.map((path) => `<li title="${escape(path)}">${escape(path)}</li>`).join("")}</ul>`
        : "";
      const root = ask.grantRoot ? `<p class="ask-reason">${escape(words.grantRoot(ask.grantRoot))}</p>` : "";
      body = `${head(ask.paths.length ? words.filesNamed(ask.paths.length) : words.files)}${paths}${reason(ask.reason)}${root}${buttons(ask.decisions)}`;
      break;
    }
    case "questions": {
      const questions = ask.questions
        .map((q, index) => {
          const options = q.options
            .map(
              (option, n) =>
                `<button type="button" class="ask-option" data-question="${index}" data-option="${n}" aria-pressed="false"><span class="ask-option-label">${escape(option.label)}</span>${option.description ? `<span class="ask-option-what">${escape(option.description)}</span>` : ""}</button>`,
            )
            .join("");
          const free =
            q.other || q.options.length === 0
              ? `<input class="ask-input" data-question="${index}" type="${q.secret ? "password" : "text"}" autocomplete="off" placeholder="${escape(q.options.length ? words.otherAnswer : q.secret ? words.secret : "")}">`
              : "";
          return `<div class="ask-question" role="group" aria-label="${escape(q.header || q.question)}">${q.header ? `<div class="ask-question-head">${escape(q.header)}</div>` : ""}<div class="ask-question-text">${plain(q.question)}</div>${options ? `<div class="ask-options">${options}</div>` : ""}${free}</div>`;
        })
        .join("");
      body = `${head(ask.questions.length > 1 ? words.questions(ask.questions.length) : words.question)}${questions}<div class="ask-actions"><button type="button" class="ask-button ask-primary" data-ask="${key}" data-submit="answer">${escape(words.submit)}</button><button type="button" class="ask-button ask-no" data-ask="${key}" data-submit="skip">${escape(words.skip)}</button></div>`;
      break;
    }
    case "form": {
      const message = ask.message ? `<div class="ask-message">${plain(ask.message)}</div>` : "";
      const fields = ask.fields ? `<div class="ask-fields">${ask.fields.map((field, index) => drawField(field, index, words)).join("")}</div>` : `<p class="ask-reason">${escape(words.formUnreadable)}</p>`;
      const submit = ask.fields ? `<button type="button" class="ask-button ask-primary" data-ask="${key}" data-submit="accept">${escape(words.fill)}</button>` : "";
      body = `${head(words.form(ask.server))}${message}${fields}<div class="ask-actions">${submit}<button type="button" class="ask-button ask-no" data-ask="${key}" data-submit="decline">${escape(words.refuse)}</button><button type="button" class="ask-button ask-no" data-ask="${key}" data-submit="cancel">${escape(words.dismiss)}</button></div>`;
      break;
    }
    case "link": {
      const message = ask.message ? `<div class="ask-message">${plain(ask.message)}</div>` : "";
      body = `${head(words.link(ask.server))}${message}<p class="ask-url">${escape(ask.url)}</p><div class="ask-actions"><button type="button" class="ask-button ask-primary" data-ask="${key}" data-submit="open">${escape(words.openLink)}</button><button type="button" class="ask-button ask-no" data-ask="${key}" data-submit="decline">${escape(words.refuse)}</button><button type="button" class="ask-button ask-no" data-ask="${key}" data-submit="cancel">${escape(words.dismiss)}</button></div>`;
      break;
    }
    case "cannot":
      body = `${head(ask.what)}${ask.detail ? `<p class="ask-reason">${escape(ask.detail)}</p>` : ""}`;
      break;
  }
  return `<div class="ask-card" role="group" data-card="${key}">${body}<p class="ask-status" aria-live="polite"></p></div>`;
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
      return `<div class="ask-field" role="group">${label}<div class="ask-options">${field.options
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
    case "files": {
      const decision = ask.decisions[given.decision ?? -1];
      if (!decision) return { missing: words.required };
      return { answer: { decision: decision.value }, said: decision.settled };
    }
    case "questions": {
      const skip = given.submit === "skip";
      const answers: Record<string, JsonValue> = {};
      for (const [index, q] of ask.questions.entries()) {
        if (skip) {
          answers[q.id] = { answers: [] };
          continue;
        }
        const picked = [...card.querySelectorAll<HTMLElement>(`.ask-option[data-question="${index}"][aria-pressed="true"]`)].map(
          (button) => q.options[Number(button.dataset.option)]!.label,
        );
        const typed = card.querySelector<HTMLInputElement>(`.ask-input[data-question="${index}"]`)?.value.trim() ?? "";
        answers[q.id] = { answers: typed ? [...picked, typed] : picked };
      }
      return { answer: { answers }, said: skip ? words.settledBy.skipped : words.settledBy.answered };
    }
    case "form": {
      if (given.submit !== "accept") return refusal(given.submit, words);
      if (!ask.fields) return { missing: words.required };
      const content: Record<string, JsonValue> = {};
      for (const [index, field] of ask.fields.entries()) {
        const value = fieldValue(card, field, index);
        if (value === null) {
          if (field.required) return { missing: `${words.required}: ${field.title}` };
          continue;
        }
        content[field.name] = value;
      }
      return { answer: { action: "accept", content, _meta: null }, said: words.settledBy.submitted };
    }
    case "link":
      if (given.submit !== "open") return refusal(given.submit, words);
      return { answer: { action: "accept", content: null, _meta: null }, said: words.settledBy.opened };
    case "cannot":
    case "hidden":
      return { missing: words.required };
  }
}

function refusal(submit: string | undefined, words: AskWords): { answer: JsonValue; said: string } {
  return submit === "cancel"
    ? { answer: { action: "cancel", content: null, _meta: null }, said: words.settledBy.dismissed }
    : { answer: { action: "decline", content: null, _meta: null }, said: words.settledBy.refused };
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
