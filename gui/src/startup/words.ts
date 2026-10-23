// The words of the page shown while Fastctx opens and when it could not.

/** The layers a repair goes through, as the repair names them. */
export type RepairLayer = "package" | "controller" | "records" | "program" | "service" | "application" | "node" | "reopen";

export interface StartupWords {
  opening: string;
  failed: string;
  /** What did not open, by the step that said so. */
  steps: Record<string, string>;
  stepOther: string;
  retry: string;
  repair: string;
  repairing: string;
  repaired: string;
  repairFailed: string;
  repairAgain: string;
  layers: Record<RepairLayer, string>;
  /** How a layer went, said when pointed at. */
  unchanged: string;
  fixed: string;
  broken: string;
  checking: string;
  /** How a layer went, then the repair's own words. */
  detail: (said: string, detail: string) => string;
}

export const zhStartup: StartupWords = {
  opening: "正在打开 Fastctx",
  failed: "Fastctx 没能打开",
  steps: {
    open_connection: "没连上这台设备上的 Fastctx。",
  },
  stepOther: "打开时出了问题。",
  retry: "重试",
  repair: "修复",
  repairing: "正在修复",
  repaired: "修好了，正在重新打开",
  repairFailed: "没修好",
  repairAgain: "再修一次",
  layers: {
    package: "取安装包",
    controller: "准备修复",
    records: "记录",
    program: "程序",
    service: "后台服务",
    application: "应用",
    node: "连接",
    reopen: "重新打开",
  },
  unchanged: "完好",
  fixed: "已修好",
  broken: "没修好",
  checking: "检查中",
  detail: (said, detail) => `${said}：${detail}`,
};

export const enStartup: StartupWords = {
  opening: "Opening Fastctx",
  failed: "Fastctx didn't open",
  steps: {
    open_connection: "Couldn't connect to Fastctx on this device.",
  },
  stepOther: "Something went wrong while opening.",
  retry: "Try again",
  repair: "Repair",
  repairing: "Repairing",
  repaired: "Repaired, opening again",
  repairFailed: "Repair didn't finish",
  repairAgain: "Repair again",
  layers: {
    package: "Package",
    controller: "Preparing",
    records: "Records",
    program: "Program",
    service: "Background service",
    application: "Application",
    node: "Connection",
    reopen: "Opening again",
  },
  unchanged: "Fine",
  fixed: "Repaired",
  broken: "Not repaired",
  checking: "Checking",
  detail: (said, detail) => `${said}: ${detail}`,
};
