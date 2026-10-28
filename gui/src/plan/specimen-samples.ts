// Plans for looking at the panel without a world behind it, with the
// world's facts about the ids they name.

import type { LiveFact } from "./live";

export interface PlanSample {
  source: string;
  version: number;
  when: string;
  facts: Record<string, LiveFact>;
}

const busy = (agent: "codex" | "claude", label: string, state: string): LiveFact => ({ kind: "resident", agent, label, state, lit: true });
const resting = (agent: "codex" | "claude", label: string, state: string): LiveFact => ({ kind: "resident", agent, label, state });
const sense = (label: string, state: string): LiveFact => ({ kind: "sense", label, state });
const job = (label: string, state: string, lit = true): LiveFact => ({ kind: "job", label, state, lit });

export const samples: Record<string, PlanSample> = {
  laptop: {
    version: 2,
    when: "今天 09:40",
    facts: {
      "s-pr1ce7": sense("两台里有一台降到 7,000 以下", "盯着"),
    },
    source: `# 买一台新电脑

> 预算八千以内，主要写代码，要轻

\`\`\`flow
列出需求 -> 两台候选
两台候选 -> 去店里试: 这周六
两台候选 --> 网上直接买: 没时间就这样
去店里试 -> 下单
网上直接买 --> 下单
\`\`\`

- [x] 列出需求
- [x] 挑出两台候选
- [ ] 这周六去店里试
- [ ] 下单

s-pr1ce7 在看价格，降了就叫醒我。

\`\`\`note info
两台的屏幕都是 14 英寸，重量差 200 克。
\`\`\`
`,
  },
  blog: {
    version: 3,
    when: "40 分钟前",
    facts: {
      "codex-b10g": busy("codex", "评论的后端", "在干活，1 小时"),
      "s-ci4b2x": sense("CI 跑完", "盯着"),
      "j-dev7k2": job("npm run dev", "在跑，2 小时"),
    },
    source: `# 给博客加上评论

文章底下能留言，作者能删。后端交给 codex-b10g，界面我来做。

\`\`\`flow
设计 -> 后端：存储与接口
后端：存储与接口 -> 界面：评论区
界面：评论区 -> 审核与删除
审核与删除 --> 邮件提醒: 以后再说
\`\`\`

- [x] 设计
- [x] 后端：存储与接口
- [ ] 界面：评论区
- [ ] 审核与删除

预览一直开着：j-dev7k2。后端推上去之后，s-ci4b2x 等 CI 跑完叫醒我。

\`\`\`ask
游客能不能直接留言？
* 先登录才能留言 | 少很多垃圾评论
- 游客也能留，作者审核后显示
\`\`\`
`,
  },
  move: {
    version: 5,
    when: "昨天 22:10",
    facts: {
      "j-rsy9q3": job("rsync 照片到 NAS", "在跑，3 小时"),
      "s-nas0k1": sense("NAS 上的照片和旧电脑一样多", "盯着"),
      "claude-ph0t": resting("claude", "照片去重", "睡着，已交回"),
    },
    source: `# 搬家前把旧电脑的数据迁到 NAS

\`\`\`timeline
10-22 | 照片 | j-rsy9q3 在传，约 410 GB
10-23 | 文档与邮件 | 照片传完再开始
10-25 | 核对后清空旧电脑
10-26 | 搬家
\`\`\`

\`\`\`tree
旧电脑
  照片 | 去重后 38,200 张
  文档 | 约 12 GB
  邮件 | 导出成 mbox
  软件 | 不迁，到新电脑上重装
\`\`\`

去重是 claude-ph0t 做的，重复的 1,140 张已移到「待删」文件夹。s-nas0k1 在 NAS 上的数目对上时叫醒我。

\`\`\`note warn
旧电脑的硬盘有坏道告警。照片传完之前别重启它。
\`\`\`
`,
  },
  essay: {
    version: 2,
    when: "3 小时前",
    facts: {
      "codex-c1ty": busy("codex", "查骑行的统计数字", "在干活，25 分钟"),
    },
    source: `# 一篇关于城市骑行的文章

> 写给本地报纸的周末版，三千字以内，周五交。

开头写早高峰骑车穿过老城的那二十分钟，中间讲数字与道路，结尾回到人。

\`\`\`flow
开头：早高峰的二十分钟 -> 中间：数字与道路 -> 结尾：回到人
\`\`\`

数字由 codex-c1ty 去查，每个都要有来源。

\`\`\`note ok
受访的三位骑手都同意署真名。
\`\`\`

\`\`\`ask
标题用哪一个？
* 二十分钟的老城
- 在车流里骑车
- 自己写一个
\`\`\`
`,
  },
};
