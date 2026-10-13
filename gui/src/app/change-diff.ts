// What hooking a login up or removing it would change in a file, read as a
// change reads in a conversation (`world-gui.md`, 账号、机器、共享与协助). The
// node names each setting that would change by its key path, with its
// values where they are the World's own; here they are written back as the
// file writes them: a TOML table's keys under its header, a JSON setting
// within its objects, the World's section of a guidance file as its lines.
// A value that might be the user's is named, never shown.

import type { FilePreview, ItemChange } from "../world/logins";

/** A path as it reads: without Windows' long-path prefix, the home as `~`. */
export function shortPath(path: string): string {
  return path
    .replace(/^\\\\\?\\(?!UNC\\)/, "")
    .replace(/^[A-Za-z]:\\Users\\[^\\]+(?=\\|$)/i, "~")
    .replace(/^\/(?:home\/[^/]+|root|Users\/[^/]+)(?=\/|$)/, "~");
}

/** The file's change as lines of a diff: "-" removed, "+" added, " " left
 *  alone; `withheld` is said beside a value not shown. */
export function previewDiff(file: FilePreview, withheld: string): string[] {
  if (/\.toml$/i.test(file.path)) return tomlLines(file.items, withheld);
  if (/\.json$/i.test(file.path)) return jsonLines(file.items, withheld);
  return file.items.flatMap((item) => [...textLines(item.before).map((line) => `-${line}`), ...textLines(item.after).map((line) => `+${line}`)]);
}

function textLines(text: string | undefined): string[] {
  if (text === undefined) return [];
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/** A key path as the node writes it, `a."b.c".d`, back into its keys. */
function keysOf(path: string): string[] {
  const keys: string[] = [];
  let at = 0;
  while (at < path.length) {
    if (path[at] === '"') {
      let end = at + 1;
      while (end < path.length && path[end] !== '"') end += path[end] === "\\" ? 2 : 1;
      keys.push(JSON.parse(path.slice(at, end + 1)) as string);
      at = end + 2;
    } else {
      const dot = path.indexOf(".", at);
      keys.push(path.slice(at, dot === -1 ? path.length : dot));
      at = dot === -1 ? path.length : dot + 1;
    }
  }
  return keys;
}

/** A value the node gives as JSON, as its JSON again or as it was given. */
function parsed(value: string): { ok: true; value: unknown } | { ok: false } {
  try {
    return { ok: true, value: JSON.parse(value) };
  } catch {
    return { ok: false };
  }
}

const bareKey = /^[A-Za-z0-9_-]+$/;

function tomlKey(key: string): string {
  return bareKey.test(key) ? key : JSON.stringify(key);
}

/** A value as TOML writes it; a Windows path in single quotes, as it is
 *  typed, rather than with every backslash doubled. */
function tomlValue(value: unknown): string {
  if (typeof value === "string") return /\\/.test(value) && !/['\n\r]/.test(value) ? `'${value}'` : JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(tomlValue).join(", ")}]`;
  if (value !== null && typeof value === "object") {
    return `{ ${Object.entries(value).map(([key, inner]) => `${tomlKey(key)} = ${tomlValue(inner)}`).join(", ")} }`;
  }
  return String(value);
}

function tomlLines(items: ItemChange[], withheld: string): string[] {
  const tables = new Map<string, ItemChange[]>();
  for (const item of items) {
    const keys = keysOf(item.item);
    const table = keys.slice(0, -1).map(tomlKey).join(".");
    tables.set(table, [...(tables.get(table) ?? []), item]);
  }
  const lines: string[] = [];
  for (const [table, inside] of tables) {
    const keys: string[] = [];
    for (const item of inside) {
      const key = tomlKey(keysOf(item.item).at(-1) ?? item.item);
      if (item.withheld) {
        keys.push(` ${key} = …  # ${withheld}`);
        continue;
      }
      const written = (value: string) => {
        const json = parsed(value);
        return `${key} = ${json.ok ? tomlValue(json.value) : value}`;
      };
      if (item.before !== undefined) keys.push(`-${written(item.before)}`);
      if (item.after !== undefined) keys.push(`+${written(item.after)}`);
    }
    if (!keys.length) continue;
    if (table) lines.push(` [${table}]`);
    lines.push(...keys);
  }
  return lines;
}

/** The settings that change, within the objects that hold them, each object
 *  opened once however many of its settings change. */
function jsonLines(items: ItemChange[], withheld: string): string[] {
  interface Node {
    children: Map<string, Node>;
    items: { key: string; item: ItemChange }[];
  }
  const root: Node = { children: new Map(), items: [] };
  for (const item of items) {
    const keys = keysOf(item.item);
    let node = root;
    for (const key of keys.slice(0, -1)) {
      let next = node.children.get(key);
      if (!next) node.children.set(key, (next = { children: new Map(), items: [] }));
      node = next;
    }
    node.items.push({ key: keys.at(-1) ?? item.item, item });
  }
  const lines: string[] = [" {"];
  const walk = (node: Node, depth: number) => {
    const pad = "  ".repeat(depth);
    for (const { key, item } of node.items) {
      const name = JSON.stringify(key);
      if (item.withheld) {
        lines.push(` ${pad}${name}: …  // ${withheld}`);
        continue;
      }
      const written = (value: string) => {
        const json = parsed(value);
        return `${pad}${name}: ${json.ok ? JSON.stringify(json.value) : value}`;
      };
      if (item.before !== undefined) lines.push(`-${written(item.before)}`);
      if (item.after !== undefined) lines.push(`+${written(item.after)}`);
    }
    for (const [key, inner] of node.children) {
      lines.push(` ${pad}${JSON.stringify(key)}: {`);
      walk(inner, depth + 1);
      lines.push(` ${pad}}`);
    }
  };
  walk(root, 1);
  lines.push(" }");
  return lines;
}
