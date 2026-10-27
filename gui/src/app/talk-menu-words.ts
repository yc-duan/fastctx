// The words of a conversation's "⋯" and of what it does (`world-gui.md`,
// 界面的结构): short, as on any product's menu; what an act means is said
// on the card that asks before it, and why one was not done in a line that
// says so once.

export interface TalkMenuWords {
  more: string;
  move: string;
  rename: string;
  /** Finding in the conversation, from its head's "⋯" on a phone. */
  find: string;
  /** Showing every step of its work, or only what was said, from the same
   *  "⋯" on a phone. */
  viewSteps: string;
  viewSaid: string;
  /** The panel beside it, from the same "⋯" on a phone. */
  panel: string;
  /** The whole conversation onto the clipboard, and that it is there. */
  copyMarkdown: string;
  copied: string;
  /** Opening it in its agent's own program on this computer, and that it
   *  opened. */
  openOwn(agent: string): string;
  openedOwn: string;
  archive: string;
  restore: string;
  delete: string;
  noProject: string;
  /** No project spans its machine. */
  noProjects: string;
  /** Its machine is away: only deleting goes ahead now. */
  offline(machine: string): string;
  archived: string;
  /** The sidebar's group of archived conversations. */
  archivedGroup: string;
  deleting: string;
  undo: string;
  /** Said once it is archived, with a way back for a moment. */
  archivedSaid(name: string): string;
  restoredSaid(name: string): string;
  movedSaid(name: string, project: string | null): string;
  /** An act refused or failed, what the World said kept for hovering. */
  notArchived: string;
  notRestored: string;
  notMoved: string;
  notRenamed: string;
  notDeleted: string;
  /** The card asking before deleting. */
  deleteTitle(name: string): string;
  deleteWhat: string;
  /** What deleting takes away, one tag each; `host` is what its agent's
   *  own program keeps. */
  deleteParts(host: string): string[];
  /** What its agent's own program keeps of it. */
  hostRecord(agent: "codex" | "claude" | string): string;
  deleteWorking: string;
  deleteAway(machine: string): string;
  deleteGo: string;
  cancel: string;
  working: string;
  failed(why: string): string;
  /** The line in place of the composer. */
  archivedNote: string;
  deletedNote: string;
}

export const zhTalkMenu: TalkMenuWords = {
  more: "更多",
  move: "移入项目",
  rename: "重命名",
  find: "查找",
  viewSteps: "显示每一步",
  viewSaid: "只看对话",
  panel: "侧面板",
  copyMarkdown: "复制为 Markdown",
  copied: "已复制",
  openOwn: (agent) => `在 ${agent} 中打开`,
  openedOwn: "已在新终端窗口中打开",
  archive: "归档",
  restore: "恢复",
  delete: "删除",
  noProject: "不放在项目里",
  noProjects: "还没有项目",
  offline: (machine) => `${machine} 离线，只能删除`,
  archived: "已归档",
  archivedGroup: "已归档",
  deleting: "正在删除",
  undo: "撤销",
  archivedSaid: (name) => `已归档「${name}」`,
  restoredSaid: (name) => `已恢复「${name}」`,
  movedSaid: (name, project) => (project ? `「${name}」已移入 ${project}` : `「${name}」已移出项目`),
  notArchived: "没能归档",
  notRestored: "没能恢复",
  notMoved: "没能移动",
  notRenamed: "没能改名",
  notDeleted: "没能删除",
  deleteTitle: (name) => `删除「${name}」？`,
  deleteWhat: "所有机器与设备上的都会删掉，拿不回来。",
  deleteParts: (host) => ["对话与附件", "各台机器上的命令与输出", host, "各设备上的副本"],
  hostRecord: (agent) => (agent === "codex" ? "Codex 的线程记录" : agent === "claude" ? "Claude Code 的会话记录" : "智能体自己的记录"),
  deleteWorking: "它正在干活，会先停下。",
  deleteAway: (machine) => `${machine} 离线，连回来后再删。`,
  deleteGo: "删除",
  cancel: "取消",
  working: "正在删除…",
  failed: (why) => `没删成：${why}`,
  archivedNote: "已归档",
  deletedNote: "对话已删除",
};

export const enTalkMenu: TalkMenuWords = {
  more: "More",
  move: "Move to project",
  rename: "Rename",
  find: "Find",
  viewSteps: "Show every step",
  viewSaid: "Show only what was said",
  panel: "Side panel",
  copyMarkdown: "Copy as Markdown",
  copied: "Copied",
  openOwn: (agent) => `Open in ${agent}`,
  openedOwn: "Opened in a new terminal window",
  archive: "Archive",
  restore: "Restore",
  delete: "Delete",
  noProject: "No project",
  noProjects: "No projects yet",
  offline: (machine) => `${machine} is offline; only deleting works`,
  archived: "Archived",
  archivedGroup: "Archived",
  deleting: "Deleting",
  undo: "Undo",
  archivedSaid: (name) => `Archived “${name}”`,
  restoredSaid: (name) => `Restored “${name}”`,
  movedSaid: (name, project) => (project ? `Moved “${name}” to ${project}` : `Took “${name}” out of its project`),
  notArchived: "Couldn't archive",
  notRestored: "Couldn't restore",
  notMoved: "Couldn't move",
  notRenamed: "Couldn't rename",
  notDeleted: "Couldn't delete",
  deleteTitle: (name) => `Delete “${name}”?`,
  deleteWhat: "It goes from every machine and device, for good.",
  deleteParts: (host) => ["Messages and attachments", "Commands and output on every machine", host, "Copies on devices"],
  hostRecord: (agent) => (agent === "codex" ? "Codex's thread" : agent === "claude" ? "Claude Code's session" : "The agent's own record"),
  deleteWorking: "It's working; it will be stopped first.",
  deleteAway: (machine) => `${machine} is offline; it's deleted there when it's back.`,
  deleteGo: "Delete",
  cancel: "Cancel",
  working: "Deleting…",
  failed: (why) => `Not deleted: ${why}`,
  archivedNote: "Archived",
  deletedNote: "This conversation was deleted",
};
