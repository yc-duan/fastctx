// The question a link to a program on this computer asks before it runs it.

export interface ProgramWords {
  /** Asking whether to run the program `name`. */
  title(name: string): string;
  /** What opening it does, in one line. */
  what: string;
  show: string;
  run: string;
  close: string;
  /** Neither came to anything; the system's words are on hover. */
  failed: string;
}

export const zhProgram: ProgramWords = {
  title: (name) => `运行 ${name}？`,
  what: "打开即运行该程序。",
  show: "在文件夹中显示",
  run: "运行",
  close: "关闭",
  failed: "操作失败",
};

export const enProgram: ProgramWords = {
  title: (name) => `Run ${name}?`,
  what: "Opening it runs the program.",
  show: "Show in folder",
  run: "Run",
  close: "Close",
  failed: "That didn't work",
};
