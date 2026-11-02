// What the agents' overview says (`agents-overview.ts`): its title, each
// card's way to the agent's own page, a login's actions, and the agents not
// found on the machine shown.

export interface AgentsOverviewWords {
  title: string;
  /** A card's way to the agent's own page, and what is there. */
  manage: string;
  manageSaid: string;
  /** A connected login's way into the workbench, and what it is. */
  workbench: string;
  workbenchSaid: string;
  /** A login whose connection was changed since, or asks for more. */
  renew: string;
  /** An agent whose program is here, with no login yet. */
  noLogin: string;
  /** The agents not found on the machine shown. */
  missing(names: string[], here: boolean): string;
}

export const zhAgentsOverview: AgentsOverviewWords = {
  title: "智能体",
  manage: "管理",
  manageSaid: "全部登录、搜索的文件夹与扫描",
  workbench: "工作台",
  workbenchSaid: "在 Fastctx 中与它对话；开启前先说明将做的改动",
  renew: "更新接入",
  noLogin: "已安装，尚无登录",
  missing: (names, here) => `${here ? "这台电脑" : "这台机器"}上未找到 ${names.join("、")}`,
};

export const enAgentsOverview: AgentsOverviewWords = {
  title: "Agents",
  manage: "Manage",
  manageSaid: "Every login, the folders searched, and scans",
  workbench: "Workbench",
  workbenchSaid: "Talk to it in Fastctx; what opening it changes is shown first",
  renew: "Update connection",
  noLogin: "Installed, no login yet",
  missing: (names, here) => `${names.join(", ")} not found on this ${here ? "computer" : "machine"}`,
};
