// The desk's words (`desk/desk.ts`), in both languages.

export interface DeskWords {
  close: string;
  /** A fork on its way, and one that waits for the turn under way. */
  forking: string;
  forkWaits: string;
  /** What pointing at the card of where a fork came from says. */
  backTo(name: string): string;
}

export const zhDesk: DeskWords = {
  close: "关闭",
  forking: "正在分叉…",
  forkWaits: "这一轮结束后分叉",
  backTo: (name) => `回到「${name}」`,
};

export const enDesk: DeskWords = {
  close: "Close",
  forking: "Forking…",
  forkWaits: "Forks when this turn ends",
  backTo: (name) => `Back to ${name}`,
};
