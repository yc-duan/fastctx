// The account's skills (`world-user-config.md`, 技能), as the shell keeps
// them at the account's hub (`src-tauri/src/skills.rs`).

import { invoke } from "@tauri-apps/api/core";

/** A skill's name and when it applies, as the list shows it. */
export interface SkillListed {
  name: string;
  description: string;
}

export interface Skill extends SkillListed {
  /** What to do, in Markdown. */
  instructions: string;
}

/** A skill's name, as every host names its own. */
export const SKILL_NAME = /^[a-z](?:[a-z0-9-]{0,62}[a-z0-9])?$/;
export const SKILL_WHEN_MOST = 300;
export const SKILL_INSTRUCTIONS_MOST = 60 * 1024;

export interface SkillCalls {
  list(hub: string, account: string): Promise<SkillListed[]>;
  read(hub: string, account: string, name: string): Promise<Skill | null>;
  save(hub: string, account: string, previous: string | null, skill: Skill): Promise<void>;
  remove(hub: string, account: string, name: string): Promise<void>;
}

export const skillCalls: SkillCalls = {
  list: (hub, account) => invoke("skills_list", { hub, account }),
  read: (hub, account, name) => invoke("skill_read", { hub, account, name }),
  save: (hub, account, previous, skill) => invoke("skill_save", { hub, account, previous, skill }),
  remove: (hub, account, name) => invoke("skill_delete", { hub, account, name }),
};

/** A SKILL.md as hosts write one: its name and description in the front
 *  matter, its instructions after it; a plain Markdown file is all
 *  instructions. */
export function skillFromFile(text: string, file: string): Partial<Skill> {
  const matter = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?/.exec(text);
  const field = (name: string) => {
    const line = matter?.[1].split(/\r?\n/).find((one) => one.startsWith(`${name}:`));
    return line?.slice(name.length + 1).trim().replace(/^["']|["']$/g, "");
  };
  const stem = file.replace(/\.(md|markdown|txt)$/i, "").toLowerCase();
  const named = field("name") ?? (stem === "skill" ? undefined : stem);
  return {
    ...(named !== undefined ? { name: named } : {}),
    ...(field("description") !== undefined ? { description: field("description")! } : {}),
    instructions: (matter ? text.slice(matter[0].length) : text).trim(),
  };
}
