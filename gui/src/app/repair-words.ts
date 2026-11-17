// The words of a repair, wherever one is started or followed: on the page
// shown when Fastctx did not open, on the update card, and in the machines'
// update view.

/** The layers a repair goes through, as the repair names them. */
export type RepairLayer = "package" | "controller" | "records" | "program" | "service" | "application" | "node" | "reopen";

export interface RepairWords {
  repair: string;
  repairing: string;
  repaired: string;
  /** Every layer was as it should be: nothing was changed. */
  fine: string;
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

export const zhRepair: RepairWords = {
  repair: "修复",
  repairing: "正在修复",
  repaired: "已修复",
  fine: "各项正常，未作更改",
  repairFailed: "修复未完成",
  repairAgain: "重新修复",
  layers: {
    package: "获取安装包",
    controller: "准备修复",
    records: "记录",
    program: "程序",
    service: "后台服务",
    application: "应用",
    node: "连接",
    reopen: "重新打开",
  },
  unchanged: "完好",
  fixed: "已修复",
  broken: "未修复",
  checking: "检查中",
  detail: (said, detail) => `${said}：${detail}`,
};

export const enRepair: RepairWords = {
  repair: "Repair",
  repairing: "Repairing",
  repaired: "Repaired",
  fine: "All fine; nothing changed",
  repairFailed: "Repair incomplete",
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
