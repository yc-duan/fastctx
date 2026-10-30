// The account's skills (`world-user-config.md`, 技能), as the shell keeps
// them at the account's hub (`src-tauri/src/skills.rs`): each a folder, its
// SKILL.md naming it and saying when it applies, beside the files it uses.
// A folder is read whole with the version it is at, and written whole back
// against that version, so a change made elsewhere meanwhile is not lost.

import { invoke } from "@tauri-apps/api/core";
import { onComputer } from "./bridge";

/** A skill's name and when it applies, as the list shows it. */
export interface SkillListed {
  name: string;
  description: string;
}

/** One file of a skill's folder: its path in the folder, written with `/`,
 *  and its bytes in base64; no bytes is an empty folder. */
export interface SkillFile {
  path: string;
  data: string | null;
  executable?: boolean;
}

/** A skill as it is kept, at `version`. */
export interface SkillFolder extends SkillListed {
  version: number;
  files: SkillFile[];
}

/** One change to a kept skill's folder: a file put in, whole, or taken
 *  out. */
export type SkillChange = { change: "put"; file: SkillFile } | { change: "delete"; path: string };

/** A folder picked on this device, read as a skill's files. */
export interface PickedFolder {
  /** The folder's own name, for a skill whose SKILL.md names none. */
  folder: string;
  files: SkillFile[];
}

/** A skill's name, as every host names its own. */
export const SKILL_NAME = /^[a-z](?:[a-z0-9-]{0,62}[a-z0-9])?$/;
export const SKILL_WHEN_MOST = 300;
/** The file every skill has, and is named by. */
export const SKILL_FILE = "SKILL.md";

export interface SkillCalls {
  list(hub: string, account: string): Promise<SkillListed[]>;
  /** The skill `name`, or null where there is none. */
  read(hub: string, account: string, name: string): Promise<SkillFolder | null>;
  /** Writes a new skill's folder whole as `name`; false where one by that
   *  name was made meanwhile. */
  write(hub: string, account: string, name: string, expected: number | null, files: SkillFile[]): Promise<boolean>;
  /** Changes the kept skill `name` at the version `expected`, a new name in
   *  its SKILL.md renaming it at once; false where it changed meanwhile. */
  edit(hub: string, account: string, name: string, expected: number, changes: SkillChange[]): Promise<boolean>;
  /** False where it changed meanwhile. */
  remove(hub: string, account: string, name: string, expected: number): Promise<boolean>;
  /** Picks a folder on this device and reads it; null when the user changed
   *  their mind. Absent where this device has no folders to pick. */
  pick?: (() => Promise<PickedFolder | null>) | undefined;
}

type Reply =
  | { result: "listed"; skills: SkillListed[] }
  | { result: "read"; snapshot: { metadata: { version: number; deleted: boolean } | null; skill: (SkillListed & { instructions: string; files?: SkillFile[] }) | null } }
  | { result: "written" }
  | { result: "conflict" };

const ask = (hub: string, account: string, request: Record<string, unknown>) => invoke<Reply>("skill_files", { hub, account, request });

export const skillCalls: SkillCalls = {
  list: async (hub, account) => {
    const reply = await ask(hub, account, { operation: "list" });
    return reply.result === "listed" ? reply.skills : [];
  },
  read: async (hub, account, name) => {
    const reply = await ask(hub, account, { operation: "read", name });
    if (reply.result !== "read") return null;
    const { metadata, skill } = reply.snapshot;
    if (!metadata || metadata.deleted || !skill) return null;
    return { name: skill.name, description: skill.description, version: metadata.version, files: skill.files ?? [textFile(SKILL_FILE, skillText(skill.name, skill.description, skill.instructions))] };
  },
  write: async (hub, account, name, expected, files) => (await ask(hub, account, { operation: "write", name, expected, files })).result === "written",
  edit: async (hub, account, name, expected, changes) => (await ask(hub, account, { operation: "edit", name, expected, changes })).result === "written",
  remove: async (hub, account, name, expected) => (await ask(hub, account, { operation: "delete", name, expected })).result === "written",
  // A phone has no folder of its own to take in.
  ...(onComputer() ? { pick: () => invoke<PickedFolder | null>("skill_folder_pick") } : {}),
};

/** What makes `before` into `after`: each file new or changed put in, each
 *  gone taken out. */
export function folderChanges(before: SkillFile[], after: SkillFile[]): SkillChange[] {
  const was = new Map(before.map((file) => [file.path, file]));
  const changes: SkillChange[] = after
    .filter((file) => {
      const old = was.get(file.path);
      return !old || old.data !== file.data || !!old.executable !== !!file.executable;
    })
    .map((file) => ({ change: "put", file }));
  const kept = new Set(after.map((file) => file.path));
  for (const file of before) if (!kept.has(file.path)) changes.push({ change: "delete", path: file.path });
  return changes;
}

/** A SKILL.md as hosts write one: its name and description in the front
 *  matter, its instructions after it. */
export function skillText(name: string, description: string, instructions: string): string {
  return `---\nname: ${JSON.stringify(name)}\ndescription: ${JSON.stringify(description)}\n---\n\n${instructions.replace(/\s+$/, "")}\n`;
}

/** What a SKILL.md says: its name and description, from the front matter,
 *  and its instructions after it; a plain Markdown file is all
 *  instructions. */
export function readSkillText(text: string): { name?: string; description?: string; instructions: string } {
  const matter = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?/.exec(text);
  const field = (name: string) => {
    const line = matter?.[1].split(/\r?\n/).find((one) => one.startsWith(`${name}:`));
    const value = line?.slice(name.length + 1).trim();
    if (value === undefined) return undefined;
    if (value.startsWith('"')) {
      try {
        return String(JSON.parse(value));
      } catch {
        // Quoted another way: taken as it is written.
      }
    }
    return value.replace(/^["']|["']$/g, "");
  };
  const name = field("name");
  const description = field("description");
  return { ...(name !== undefined ? { name } : {}), ...(description !== undefined ? { description } : {}), instructions: (matter ? text.slice(matter[0].length) : text).trim() };
}

/** `text` as a file of a skill's folder. */
export function textFile(path: string, text: string): SkillFile {
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  for (let at = 0; at < bytes.length; at += 0x8000) binary += String.fromCharCode(...bytes.subarray(at, at + 0x8000));
  return { path, data: btoa(binary) };
}

/** A file's text, or null where its bytes are not text. */
export function fileText(file: SkillFile): string | null {
  if (file.data === null) return null;
  try {
    const binary = atob(file.data);
    const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
    if (bytes.includes(0)) return null;
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}

/** How many bytes a file holds. */
export function fileSize(file: SkillFile): number {
  if (!file.data) return 0;
  const padding = file.data.endsWith("==") ? 2 : file.data.endsWith("=") ? 1 : 0;
  return (file.data.length * 3) / 4 - padding;
}
