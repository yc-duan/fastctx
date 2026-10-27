// A world to look at the front page with: what `fastctx w`, `nodes` and
// `accounts` would answer on a desktop one morning, and what happened
// overnight. `?accounts=2` logs it in to a second hub as well, to see the
// interface grouped by account.

import type { Address } from "../world/events";
import { LASTING, type AccountChange, type HubAccounts, type MemberSeen, type Paths, type Reach } from "../world/accounts";
import type { About, Action, HubNodes, LinkState, MachineFacts, MachineWho, Node, ProjectRecord, ResidentNow } from "../world/world";
import type { Away, WorldState } from "../home/model";
import type { AgentFacts, WorldAgents } from "../world/agents";
import type { WorldNow } from "../world/bridge";
import { specimenRemoved } from "./machine-log";
import { runningOn } from "./versions";

const at = (time: string) => `2026-10-04T${time}Z`;
/** An assistance's deadline some days from the moment the specimen is opened, at
 *  that day's six in the evening, so it is always ahead. */
const daysAhead = (days: number) => {
  const until = new Date();
  until.setDate(until.getDate() + days);
  until.setHours(18, 0, 0, 0);
  return until.toISOString();
};

const claude = (version = "2.1.289"): About["host"] => ({ agent: "claude", client: "claude-code", version });
const codex = (): About["host"] => ({ agent: "codex", client: "codex-mcp-client", version: "0.160.0" });

function resident(
  id: string,
  machine: string,
  host: About["host"],
  town: string | null,
  model: string,
  born: string,
  state: Partial<ResidentNow> & { origin?: About["origin"]; focus?: About["focus"]; forked_from?: About["forked_from"] },
): ResidentNow {
  return {
    about: {
      id,
      host,
      machine,
      born_at: at(born),
      origin: state.origin ?? { kind: "user" },
      session: null,
      world_started: true,
      model,
      acting: machine === "desktop" ? { kind: "here" } : { kind: "accounts", accounts: [{ hub: HOME, account: "me" }] },
      ...(state.focus ? { focus: state.focus } : {}),
      ...(state.forked_from ? { forked_from: state.forked_from } : {}),
    },
    town,
    here: state.here ?? true,
    now: state.now ?? null,
    last_action: state.last_action ?? null,
    moved: state.moved ?? null,
  };
}

const action = (time: string, machine: string, hand: string, what: string, result: string): Action => ({
  at: at(time),
  machine,
  hand,
  what,
  result,
});

const address = (machine: string, resident: string): Address => ({ machine, resident });

function facts(name: string, os: string, home: string): MachineFacts {
  // As a node says: a Linux machine is a server, the laptop has a battery.
  const form = os === "linux" ? "server" : name === "laptop" ? "laptop" : "desktop";
  return {
    name, os, arch: "x86_64", home, temp: os === "windows" ? `${home}/AppData/Local/Temp` : "/tmp",
    shell: "", path_separator: os === "windows" ? "\\" : "/", case_sensitive: os !== "windows", version: runningOn(name), form,
    ...(name === "gpu" ? {} : { stats: true }),
  };
}

function record(name: string, what: string, machines: string[]): ProjectRecord {
  return { version: 1, name, what, machines, places: [], canon: null, by: { kind: "user" }, on: "desktop", at: at("00:00:00") };
}

const HOME = "w-5be1fcee3751dc7a";
/** The key of the code 「机房 A」, by which two managed machines came in. */
const LAB_CODE = "k7Qp2mVx9TfR4sLw8NcB3hJd6YgZ1aEu5oKi0nWq";
const ACME = "w-9a07c2d4e1b36f58";

function node(name: string, online: boolean | null, since: string, owner: string, os: string, home: string, rights: [string, boolean, boolean], through: string | null = null): Node {
  return {
    name,
    written: name,
    online,
    since: at(since),
    through,
    facts: online ? { Ok: facts(name, os, home) } : { Err: "it is offline" },
    owner,
    rights: [[rights[0], { hands: rights[1], agents: rights[2] }]],
  };
}

/** With `?many` in the address, a team's network of about a hundred
 *  machines: twenty-five more of the account's own and others' shared with
 *  it, every seventh out of reach. */
interface ManyOne {
  name: string;
  owner: string;
  online: boolean;
  group: string | null;
}

/** The machines `?many` and `?scale` add. */
function manyOnes(): ManyOne[] {
  return [...teamOnes(), ...scaleOnes()];
}

/** The machines `?many` adds: the user's own in the groups a school's would
 *  be in, most of them away at any hour, and the members' by team. */
function teamOnes(): ManyOne[] {
  if (!new URLSearchParams(location.search).has("many")) return [];
  const ones: ManyOne[] = [];
  const floors: [string, string[]][] = [
    ["教学楼2层", ["A1", "A2", "A3", "A4", "A5", "B1", "B2", "B3", "B4", "B5", "C1", "C2", "C3", "C4", "D1", "D2", "D3", "D4", "E1", "E2"]],
    ["教学楼3层", ["AG4", "AG5", "AG6", "AG7", "AG8", "AG9", "AG10", "AG11", "AG12", "B2", "B3", "B4", "教师机"]],
    ["实验楼3层", Array.from({ length: 24 }, (_, n) => `AG${n + 1}`)],
  ];
  let index = 0;
  for (const [floor, seats] of floors) {
    for (const seat of seats) {
      index += 1;
      ones.push({ name: `${floor}${seat}`, owner: "me", online: index % 9 === 0, group: floor });
    }
  }
  const teams: [string, number][] = [["ada", 12], ["bo", 9]];
  for (const [owner, count] of teams) {
    for (let n = 1; n <= count; n += 1) {
      index += 1;
      ones.push({ name: `${owner}-${String(n).padStart(2, "0")}`, owner, online: index % 4 !== 0, group: null });
    }
  }
  return ones;
}

/** With `?scale=<n>`, a network of about n machines more, the size a
 *  network of thousands reaches: a hundred of the account's own in a
 *  school's rooms, the rest by members in teams of a few to a few dozen,
 *  every tenth of them keeping a lab of a hundred; two in five out of reach.
 *  The same every time it is opened. */
function scaleOnes(): ManyOne[] {
  const wanted = Number(new URLSearchParams(location.search).get("scale") ?? 0);
  if (!(wanted > 0)) return [];
  let seed = 20261027;
  const next = () => {
    seed = (seed * 1103515245 + 12345) % 2147483648;
    return seed / 2147483648;
  };
  const ones: ManyOne[] = [];
  const rooms = ["实验楼1层", "实验楼2层", "图书馆", "行政楼", "宿舍区"];
  for (let n = 0; n < Math.min(100, wanted); n += 1) {
    const room = rooms[n % rooms.length]!;
    ones.push({ name: `${room}-${String(Math.floor(n / rooms.length) + 1).padStart(2, "0")}`, owner: "me", online: next() > 0.4, group: room });
  }
  const people = scalePeople();
  let index = 0;
  while (ones.length < wanted && index < people.length) {
    const person = people[index]!;
    index += 1;
    const lab = index % 10 === 0;
    const count = lab ? 100 : 1 + Math.floor(next() * 30);
    for (let k = 1; k <= count && ones.length < wanted; k += 1) {
      const group = lab ? "机房" : count > 12 && k > count / 2 ? "训练" : null;
      ones.push({ name: `${person}-${lab ? "lab" : "pc"}${String(k).padStart(3, "0")}`, owner: person, online: next() > 0.4, group });
    }
  }
  return ones;
}

/** The members `?scale` adds: enough of them for the machines it asks for. */
function scalePeople(): string[] {
  const wanted = Number(new URLSearchParams(location.search).get("scale") ?? 0);
  if (!(wanted > 0)) return [];
  const names = ["chen", "li", "wang", "zhang", "liu", "yang", "huang", "zhao", "wu", "zhou", "xu", "sun", "ma", "zhu", "hu", "guo", "he", "lin", "gao", "luo"];
  return Array.from({ length: Math.ceil(wanted / 20) + 10 }, (_, n) => `${names[n % names.length]}${Math.floor(n / names.length) + 1}`);
}

/** What each machine `?scale` adds answers with: nothing at work on it,
 *  or that it is away. */
function scaleWho(): MachineWho[] {
  return scaleOnes().map((one) =>
    one.online
      ? { kind: "answered" as const, reply: { machine: one.name, residents: [], jobs: [] } }
      : { kind: "offline" as const, machine: one.name, since: at("01:00:00") },
  );
}

/** With `?talks=<n>`, n conversations more of the account's, as many as a
 *  heavy user keeps over months: in the two projects and outside them, each
 *  idle since a little earlier than the last, now and then one waiting on
 *  the reader. */
function talking(): ResidentNow[] {
  const wanted = Number(new URLSearchParams(location.search).get("talks") ?? 0);
  if (!(wanted > 0)) return [];
  const towns = ["mark50", "ingest", null];
  const subjects = ["日志轮转", "批量提交", "图标导出", "PID 环改定点", "Fix the flaky upload test", "迁移到 SQLite", "Release notes", "缓存命中率", "Refactor the parser", "部署脚本"];
  const start = Date.parse(at("13:00:00"));
  return Array.from({ length: wanted }, (_, n) =>
    resident(`codex-${(0x10000 + n).toString(16)}`, "laptop", codex(), towns[n % towns.length]!, "gpt-6.1-sol", "00:00:00", {
      now: {
        owed: {},
        answering: [],
        activity: "idle",
        since: new Date(start - n * 7 * 60_000).toISOString(),
        turn: null,
        trouble: null,
        desk: false,
        waiting_for_user: n % 499 === 0,
        title: `${subjects[n % subjects.length]} ${n + 1}`,
      },
    }),
  );
}

/** With `?s=tour`, the Claude of that conversation (`tour.ts`), at work
 *  across four machines. */
function touring(): ResidentNow[] {
  if (new URLSearchParams(location.search).get("s") !== "tour") return [];
  return [
    resident("claude-t0ur", "laptop", claude(), "mark50", "claude-opus-5-5", "14:10:00", {
      focus: {
        machines: [
          { machine: "laptop", folders: ["C:/Users/me/mark50/firmware"] },
          { machine: "lab", folders: ["/srv/mark50"] },
          { machine: "server", folders: ["/srv/telemetry"] },
          { machine: "gpu", folders: ["/home/bo/runs"] },
        ],
      },
      now: { owed: {}, answering: [], activity: "idle", since: at("14:38:00"), turn: null, trouble: null, desk: false, waiting_for_user: false, title: "肘关节前馈" },
    }),
  ];
}

/** With `?s=voice`, the conversation that worked through the night
 *  (`voice.ts`), the last it told the user an urgent one. */
function voicing(): ResidentNow[] {
  if (new URLSearchParams(location.search).get("s") !== "voice") return [];
  return [
    resident("claude-v01c", "laptop", claude(), "mark50", "claude-opus-5-5", "14:10:00", {
      now: {
        owed: {},
        answering: [],
        activity: "idle",
        since: at("16:58:20"),
        turn: null,
        trouble: null,
        desk: false,
        waiting_for_user: false,
        title: "夜里的回归与训练",
        told: { id: "told-3", at: at("16:41:30"), status: "urgent", text: "训练在 11,200 步之后发散了，loss 从 0.31 升到 4.7。我停了它，从第 11k 步的检查点把学习率减半重跑（run3b），前一千步正常。要不要把这个学习率写进默认配置，等你定。", turn: "t3" },
      },
    }),
  ];
}

/** With `?s=project`, mark50's other conversations (`hub.ts`): one that
 *  worked through the night and told the user something urgent, one that
 *  asks the user, and one asleep on the desktop with something told that
 *  can wait. */
function hubbing(machine: "laptop" | "desktop"): ResidentNow[] {
  if (new URLSearchParams(location.search).get("s") !== "project") return [];
  if (machine === "desktop") {
    return [
      resident("codex-4c1e", "desktop", codex(), "mark50", "gpt-6.1-sol", "09:30:00", {
        now: {
          owed: {},
          answering: [],
          activity: "asleep",
          since: at("12:40:00"),
          turn: null,
          trouble: null,
          desk: false,
          waiting_for_user: false,
          title: "CAN 帧时间戳",
          told: { id: "told-4c1e-2", at: at("12:38:00"), status: "info", text: "驱动层打戳改好了，应用层的去重删掉了；回放 42 帧，时间戳全部单调。", turn: "t6" },
        },
      }),
    ];
  }
  return [
    resident("claude-v01c", "laptop", claude(), "mark50", "claude-opus-5-5", "02:10:00", {
      now: {
        owed: {},
        answering: [],
        activity: "idle",
        since: at("04:58:20"),
        turn: null,
        trouble: null,
        desk: false,
        waiting_for_user: false,
        title: "夜里的回归与训练",
        told: { id: "told-v01c-3", at: at("04:41:30"), status: "urgent", text: "训练在 11,200 步之后发散了，loss 从 0.31 升到 4.7。我停了它，从第 11k 步的检查点把学习率减半重跑（run3b），前一千步正常。要不要把这个学习率写进默认配置，等你定。", turn: "t3" },
      },
    }),
    resident("claude-a7e2", "laptop", claude(), "mark50", "claude-opus-5-5", "10:05:00", {
      now: {
        owed: {},
        answering: [],
        activity: "idle",
        since: at("13:12:00"),
        turn: null,
        trouble: null,
        desk: false,
        waiting_for_user: false,
        title: "真机验证",
        asked: { id: "ask-a7e2-1", at: at("13:12:00"), waits: false, what: "answer", text: "周五借实验室的真机跑 50 次，还是先在台架上跑？", options: ["周五借真机", "先上台架"] },
      },
    }),
  ];
}

function many(): Node[] {
  return manyOnes().map((one, index) => {
    const linux = index % 3 !== 0;
    return node(one.name, one.online, "01:00:00", one.owner, linux ? "linux" : "windows", linux ? `/home/${one.owner}` : `C:/Users/${one.owner}`, ["me", true, true]);
  });
}

/** The hubs this desktop is logged in to, as `nodes` and `accounts` see
 *  them: its own, where it is the super administrator, has shared lab with
 *  ada and lets a Codex login of this computer's assist ada, once used an
 *  agent on bo's gpu, keeps two managed machines that came in by the code
 *  「机房 A」, and has cy as a visitor, whose mac it sees only by name;
 *  and with `two`, a second one at work, where it is a member that ops
 *  shared build with and whom the Claude on render assists. */
function hubs(two: boolean): { nodes: HubNodes[]; accounts: HubAccounts[] } {
  // With ?hub=down, the line to home is down and reconnecting: its view
  // still has the machines, and the node leaves unsaid who is online now.
  const down = new URLSearchParams(location.search).get("hub") === "down";
  const link: LinkState = down
    ? { kind: "connecting", since: at("13:41:20"), last_error: "its own line: 203.0.113.10:7443 card Ethernet: connection refused" }
    : { kind: "online", since: at("01:10:02"), hub: "203.0.113.10:7443", route: { kind: "card", name: "Ethernet" } };
  const nodes: HubNodes[] = [
    {
      hub: HOME,
      name: "home",
      link,
      logins: ["me"],
      home: "me",
      machines: [
        node("laptop", true, "02:04:00", "me", "windows", "C:/Users/me", ["me", true, true]),
        node("lab", true, "00:00:00", "me", "linux", "/home/me", ["me", true, true], "server"),
        node("server", true, "00:00:00", "me", "linux", "/root", ["me", true, true]),
        node("studio", false, "12:52:00", "me", "windows", "C:/Users/me", ["me", true, true]),
        node("gpu", true, "03:30:00", "bo", "linux", "/home/bo", ["me", true, true], "la"),
        node("lab-a-01", true, "11:02:00", "me", "windows", "C:/Users/lab", ["me", true, false]),
        node("lab-a-02", false, "12:31:00", "me", "windows", "C:/Users/lab", ["me", true, false]),
        ...many(),
      ].map((machine) => (down ? { ...machine, online: null } : machine)),
    },
  ];
  // How the lines go: desktop and server take direct connections; laptop
  // and lab hold links with server, laptop one with desktop on the same
  // network, and lab's with desktop is refused by desktop's firewall.
  const paths: Record<string, Paths> = {
    desktop: { hub_ms: 41 },
    laptop: { hub_ms: 46, links: [{ peer: "server", up: true, ms: 9, since: at("02:05:00") }, { peer: "desktop", up: true, ms: 2, since: at("02:05:00") }] },
    lab: { hub_ms: 12, links: [{ peer: "server", up: true, ms: 4, since: at("00:31:00") }, { peer: "desktop", up: false, since: at("00:31:00"), why: "desktop 的防火墙没放行 7444" }] },
    server: { hub_ms: 3 },
  };
  const reach: Record<string, Reach> = {
    lab: { seen: "198.51.100.7:52114", interfaces: ["10.0.0.5", "192.168.1.30"] },
    laptop: { seen: "203.0.113.77:61002", interfaces: ["192.168.1.41"] },
  };
  const member = (name: string, kind: "computer" | "phone", logins: string[], home: string | null, added: string, online: boolean, key: string, direct: string[] = []): MemberSeen => ({
    name, kind, logins, home, direct, added: at(added), online, since: at(added), through: null, key,
    ...(paths[name] ? { paths: paths[name] } : {}),
    ...(reach[name] ? { reach: reach[name] } : {}),
  });
  const accounts: HubAccounts[] = [
    {
      hub: HOME,
      name: "home",
      link,
      logins: ["me"],
      home: "me",
      self_registration: false,
    addresses: ["192.168.1.20:7443"],
      me: "desktop",
      renames: true,
      managed: true,
      roles: true,
      groups: true,
      dissolves: true,
      deletes_projects: true,
      lan_relays: true,
      versions: true,
      due: true,
      uninstalls: true,
      accounts: [
        { name: "me", level: "super", locked: false, opened: at("00:00:00") },
        { name: "ada", level: "member", locked: false, opened: at("00:05:00") },
        { name: "bo", level: "member", locked: false, opened: at("00:06:00") },
        { name: "cy", level: "visitor", locked: false, opened: at("09:12:00") },
        ...scalePeople().map((name) => ({ name, level: "member" as const, locked: false, opened: at("00:07:00") })),
      ],
      members: [
        member("desktop", "computer", ["me"], "me", "00:10:00", true, "5be1fcee", ["192.168.1.20:7444"]),
        member("laptop", "computer", ["me"], "me", "00:20:00", true, "a1c3e9d2"),
        { ...member("lab", "computer", ["me"], "me", "00:30:00", true, "7d0e44b1"), was: ["lab-old"] },
        { ...member("server", "computer", ["me"], "me", "00:00:00", true, "c0ffee12", ["203.0.113.20:7444"]), was: ["izbp1abc0x2y3z"] },
        { ...member("studio", "computer", ["me"], "me", "00:40:00", false, "9e8d7c6b"), group: "工作室" },
        member("pixel", "phone", ["me"], null, "13:20:00", true, "4f2a91aa"),
        { ...member("gpu", "computer", ["bo"], "bo", "03:30:00", true, "be3f0a77"), group: "训练" },
        { ...member("lab-a-01", "computer", [], "me", "11:02:00", true, "31d0c8e5"), managed: true, code: LAB_CODE, group: "机房 A" },
        { ...member("lab-a-02", "computer", [], "me", "12:31:00", false, "8a4b2f60"), managed: true, code: LAB_CODE, group: "机房 A" },
        member("cy-mac", "computer", ["cy"], "cy", "09:20:00", true, "e7c13d52"),
        member("bo-phone", "phone", ["bo"], null, "14:02:00", true, "6c1d9b03"),
        member("cy-phone", "phone", ["cy"], null, "09:25:00", true, "d4e8a217"),
        ...manyOnes().map((one, index) => {
          const seen = member(one.name, "computer", [one.owner], one.owner, "01:00:00", one.online, `m${String(index).padStart(7, "0")}`);
          return one.group ? { ...seen, group: one.group } : seen;
        }),
      ],
      tokens: [
        { key: LAB_CODE, account: "me", label: "机房 A", kind: "computer", managed: true, uses: 40, used: 2, expires: daysAhead(6), way: "paste", made_by: "desktop", made_at: at("10:40:00") },
        { key: "specimen-gpu-line", account: "me", label: "GPU 集群", kind: "computer", uses: 0, used: 0, expires: LASTING, way: "line", made_by: "laptop", made_at: at("09:12:00") },
        { key: "specimen-phone-qr", account: "me", kind: "phone", uses: 1, used: 0, expires: daysAhead(1), way: "scan", made_by: "desktop", made_at: at("13:05:00") },
        { key: "specimen-bo-code", account: "bo", label: "实验室", kind: "computer", managed: true, uses: 10, used: 3, expires: daysAhead(20), way: "paste", made_by: "gpu", made_at: at("08:00:00") },
      ],
      // With ?many, a room of machines shared one by one, as a lab is.
      shares: [
        { machine: "lab", account: "ada" },
        { machine: "lab-a-01", account: "bo" },
        { machine: "lab-a-02", account: "bo" },
        ...manyOnes()
          .filter((one) => one.owner === "me" && one.group !== "教学楼2层")
          .map((one) => ({ machine: one.name, account: "bo" })),
      ],
      assists: [
        { machine: "desktop", account: "ada", agent: "codex", home: "C:\\Users\\me\\.codex-work", until: daysAhead(3) },
        { machine: "desktop", account: "bo", agent: "codex", home: "C:\\Users\\me\\.codex-work", until: null },
        { machine: "desktop", account: "ada", agent: "claude", home: null, until: daysAhead(6), weekly: 20_000_000, daily: 5_000_000, five_hour: 80 },
        { machine: "desktop", account: "bo", agent: "claude", home: null, until: null, five_hour: 80 },
      ],
      plans: [],
      acting: [{ machine: "gpu", account: "me", since: at("03:42:00") }],
      projects: [
        { name: "mark50", owner: "me", granted: ["ada"] },
        ...(two ? [] : [{ name: "ingest", owner: "me", granted: [] }]),
      ],
      arrivals: [
        { hub: HOME, account: "me", name: "pixel", device: true, noticed_at: at("13:20:00") },
        { hub: HOME, account: "me", name: "lab-a-01", device: false, noticed_at: at("11:02:00"), code: LAB_CODE },
        { hub: HOME, account: "me", name: "lab-a-02", device: false, noticed_at: at("12:31:00"), code: LAB_CODE },
      ],
    },
  ];
  if (two) {
    const work = { kind: "online" as const, since: at("01:10:05"), hub: "198.51.100.7:7443", route: { kind: "card" as const, name: "Ethernet" } };
    nodes.push({
      hub: ACME,
      name: "acme",
      link: work,
      logins: ["yc"],
      home: null,
      machines: [node("build", true, "00:05:00", "ops", "linux", "/home/ops", ["yc", true, true])],
    });
    accounts.push({
      hub: ACME,
      name: "acme",
      link: work,
      logins: ["yc"],
      home: null,
      self_registration: false,
    addresses: ["192.168.1.20:7443"],
      accounts: [
        { name: "ops", level: "super", locked: false, opened: at("00:00:00") },
        { name: "yc", level: "member", locked: false, opened: at("00:01:00") },
      ],
      members: [
        member("desktop", "computer", ["yc"], null, "00:12:00", true, "5be1fcee"),
        member("build", "computer", ["ops"], "ops", "00:05:00", true, "0b5e7a11"),
        member("render", "computer", ["ops"], "ops", "00:07:00", true, "4e11d3a9"),
      ],
      shares: [{ machine: "build", account: "yc" }],
      assists: [{ machine: "render", account: "yc", agent: "claude", home: null, until: daysAhead(1), weekly: 40_000_000, daily: 10_000_000, five_hour: 80 }],
      plans: [{ machine: "render", agent: "claude", home: null, plan: "max" }],
      acting: [{ machine: "build", account: "yc", since: at("09:15:00") }],
      projects: [{ name: "ingest", owner: "yc", granted: [] }],
      arrivals: [],
    });
  }
  return { nodes, accounts };
}

export function specimenWorld(): { world: WorldState; now: Date; lastVisit: Date } {
  const world: WorldState = {
    here: "desktop",
    // Filled from what `nodes` answers, as the shell's World is.
    nodes: [],
    assisting: new Map(),
    known: [],
    machines: [
      {
        kind: "answered",
        reply: {
          machine: "laptop",
          busy: 2,
          residents: [
            resident("claude-2b1c", "laptop", claude(), "mark50", "claude-opus-5-5", "05:02:00", {
              now: { owed: {}, answering: [], activity: "busy", since: at("13:36:00"), turn: "t3", trouble: null, desk: false, waiting_for_user: false, title: "PID 环改定点" },
              last_action: action("13:37:10", "lab", "run", "cd /srv/mark50 && git fetch laptop colleague/q16-edge && cargo test -p ctrl overflow_repro", "running"),
            }),
            resident("codex-91c0", "laptop", codex(), "mark50", "gpt-6.1-sol", "05:20:00", {
              origin: { kind: "colleague", asked_by: address("laptop", "claude-2b1c"), label: "造出让 Q32 积分溢出的输入" },
              here: false,
              now: { owed: {}, answering: [], activity: "asleep", since: at("13:34:00"), turn: null, trouble: null, desk: false, waiting_for_user: false, title: null },
              last_action: action("13:33:40", "laptop", "run", "cargo test -p ctrl overflow_repro", "exit 101"),
            }),
            ...touring(),
            ...voicing(),
            ...hubbing("laptop"),
            ...talking(),
            resident("codex-7f3a", "laptop", codex(), "ingest", "gpt-6.1-sol", "08:40:00", {
              focus: { machines: [{ machine: "lab", folders: ["/srv/ingest", "/srv/ingest/fixtures"] }, { machine: "laptop", folders: [] }] },
              now: {
                owed: {},
                answering: [],
                activity: "idle",
                since: at("09:31:00"),
                turn: null,
                trouble: null,
                desk: false,
                waiting_for_user: false,
                title: null,
                last_said: "Done. The writer now commits every 5 000 rows and at end of file, and a retry replays only the rows since the last commit.",
                asked: { id: "ask-7f3a-1", at: at("09:30:20"), waits: false, what: "answer", text: "重试时，失败的那一批也要补发告警吗？", options: ["补发", "不补发"] },
              },
              last_action: action("09:30:50", "laptop", "run", "cd /srv/ingest && cargo test 2>&1 | tail -3", "exit 0"),
            }),
          ],
          jobs: [],
        },
      },
      {
        kind: "answered",
        reply: {
          machine: "desktop",
          busy: 1,
          residents: [
            resident("codex-5e2a", "desktop", codex(), "ingest", "gpt-6.1-sol", "12:58:00", {
              now: {
                owed: {},
                answering: [],
                activity: "busy",
                since: at("13:20:00"),
                turn: "t2",
                trouble: null,
                desk: false,
                waiting_for_user: true,
                waiting_since: at("13:21:40"),
                title: "批量提交",
                asked: { id: "ask-5e2a-1", at: at("13:21:40"), waits: true, what: "command", text: "docker compose -f deploy/compose.yaml up -d ingest" },
              },
              last_action: action("13:21:30", "desktop", "apply_patch", "D:/work/ingest/deploy/compose.yaml", "1 file"),
            }),
            resident("codex-6b19", "desktop", codex(), null, "gpt-6.1-sol", "10:58:00", {
              forked_from: { machine: "desktop", resident: "codex-5a11" },
              now: { owed: {}, answering: [], activity: "idle", since: at("11:20:00"), turn: null, trouble: null, desk: false, waiting_for_user: false, title: "图标导出" },
            }),
            ...hubbing("desktop"),
          ],
          jobs: [],
        },
      },
      {
        kind: "answered",
        reply: {
          machine: "lab",
          residents: [
            resident("codex-0d1e", "lab", codex(), "mark50", "gpt-6.1-sol", "06:10:00", {
              origin: { kind: "colleague", asked_by: address("laptop", "claude-2b1c"), label: "训练关节模型 400 轮" },
              now: { owed: {}, answering: [], activity: "busy", since: at("13:12:00"), turn: "t5", trouble: { at: at("13:30:00"), text: "Reconnecting... 2/5" }, desk: false, waiting_for_user: false, title: null },
              last_action: action("13:31:00", "lab", "run", "tail -n 40 /data/mark50-sim/train.log", "exit 0"),
            }),
          ],
          jobs: [
            {
              job: "j-q5r6s7",
              machine: "lab",
              owner: address("laptop", "claude-2b1c"),
              started_at: at("13:33:00"),
              pid: 48977,
              command: "cd /srv/mark50 && git fetch laptop colleague/q16-edge && cargo test -p ctrl overflow_repro",
              output: "/home/me/.fastctx/world/towns/mark50/residents/claude-2b1c/out/j-q5r6s7.log",
            },
            {
              job: "j-p0q1r2",
              machine: "lab",
              owner: address("lab", "codex-3c77"),
              started_at: at("07:18:00"),
              pid: 48121,
              command: "python train.py --epochs 400 --joint all",
              output: "/home/me/.fastctx/world/towns/mark50/residents/codex-3c77/out/j-p0q1r2.log",
            },
            // lab is shared with bob too: what bob runs there shows, marked as his.
            {
              job: "j-b0b7e1",
              machine: "lab",
              owner: address("bob-laptop", "codex-8e21"),
              started_at: at("11:05:00"),
              pid: 50277,
              command: "python finetune.py --gpu 0 --steps 20000",
              output: "/home/me/.fastctx/world/outside/residents/codex-8e21@bob-laptop/out/j-b0b7e1.log",
              account: { hub: "w-0123456789abcdef", account: "bob" },
            },
          ],
        },
      },
      {
        kind: "answered",
        reply: {
          machine: "server",
          residents: [
            resident("claude-41aa", "server", claude(), null, "claude-opus-5-5", "04:58:00", {
              now: {
                owed: {},
                answering: [],
                activity: "idle",
                since: at("05:50:00"),
                turn: null,
                trouble: null,
                desk: false,
                waiting_for_user: false,
                title: "日志轮转",
                last_said: "Rotation now keeps 14 days of logs and compresses anything older than a day; the cron entry is in /etc/cron.d/app-logs.",
                told: { id: "told-41aa-1", at: at("05:49:40"), status: "info", text: "日志轮转改好了：留 14 天，超过一天的压缩，定时任务在 /etc/cron.d/app-logs。", turn: "t2" },
              },
            }),
          ],
          jobs: [],
        },
      },
      { kind: "offline", machine: "studio", since: at("12:52:00") },
    ],
    others: [{ machine: "lab", id: "j-b0b7e1", accounts: ["bob"] }],
    towns: [
      { town: "mark50", record: record("mark50 研发", "为 Mark 50 战衣研发纳米单元的控制固件，并在真机上验证。", ["desktop", "laptop", "lab"]), journals: 7 },
      { town: "ingest", record: record("ingest 服务", "把传感器日志批量写进时序库的服务，跑在 lab 上。", ["laptop", "desktop", "lab"]), journals: 2 },
    ],
    away: {
      unasked: [{ machine: "studio", offline: true, why: null }],
      trouble: null,
      happenings: [
      { kind: "machine", at: at("12:52:00"), machine: "studio", online: false },
      { kind: "turn_ended", at: at("13:34:00"), who: address("laptop", "codex-91c0"), town: "mark50", how: "completed", reason: null, said: "Found one. With ki = 0x7FFF_0000 and a constant error of 32767 at dt = 1, narrow() wraps instead of saturating." },
      { kind: "job_ended", at: at("13:09:12"), job: "j-k3x9a2", machine: "lab", owner: address("laptop", "claude-2b1c"), town: "mark50", how: "exited 0" },
      { kind: "asked", at: at("13:09:40"), who: address("laptop", "claude-2b1c"), colleague: address("laptop", "codex-91c0"), town: "mark50", label: "造出让 Q32 积分溢出的输入" },
      { kind: "turn_ended", at: at("09:31:00"), who: address("laptop", "codex-7f3a"), town: "ingest", how: "completed", reason: null, said: "Done. The writer now commits every 5 000 rows and at end of file, and a retry replays only the rows since the last commit." },
      { kind: "turn_ended", at: at("08:52:00"), who: address("laptop", "codex-7f3a"), town: "ingest", how: "failed", reason: "stream disconnected before completion", said: null },
      { kind: "turn_ended", at: at("11:20:00"), who: address("desktop", "codex-6b19"), town: null, how: "failed", reason: "usage limit reached; resets at 15:00", said: null },
      { kind: "turn_ended", at: at("05:50:00"), who: address("server", "claude-41aa"), town: null, how: "completed", reason: null, said: "Rotation now keeps 14 days of logs and compresses anything older than a day; the cron entry is in /etc/cron.d/app-logs." },
      { kind: "job_ended", at: at("06:05:00"), job: "j-m8v2t0", machine: "lab", owner: address("lab", "codex-0d1e"), town: "mark50", how: "exited 1" },
      ],
    },
  };
  return { world, now: new Date(at("13:40:00")), lastVisit: new Date(at("04:30:00")) };
}

/** This desktop as a managed machine of me on home (`world-network.md`,
 *  受管机器): logged in to nothing, seeing only itself; taken off the
 *  network when `removed`. */
/** The world with bo's gpu shared whole with this computer's account too,
 *  as `?shared`: a machine of another's it manages with its owner. */
export function specimenAsShared(world: WorldNow): WorldNow {
  return {
    ...world,
    accounts: {
      ...world.accounts,
      hubs: world.accounts.hubs.map((hub) => (hub.hub === HOME ? { ...hub, shares: [...hub.shares, { machine: "gpu", account: "me" }, { machine: "gpu", account: "ada" }] } : hub)),
    },
  };
}

export function specimenAsManaged(world: WorldNow, removed = false): WorldNow {
  const home = world.accounts.hubs.find((hub) => hub.hub === HOME);
  if (!home) return world;
  const me = home.members.find((member) => member.name === world.here);
  const link: LinkState = removed ? { kind: "stopped", reason: "removed by me" } : home.link;
  const hub: HubAccounts = {
    ...home,
    link,
    logins: [],
    home: "me",
    accounts: home.accounts.filter((account) => account.name === "me"),
    members: me ? [{ ...me, logins: [], managed: true, code: LAB_CODE }] : [],
    shares: [],
    assists: [],
    plans: [],
    acting: [],
    projects: [],
    arrivals: [],
    tokens: [],
  };
  return { ...world, accounts: { ...world.accounts, hubs: [hub] }, nodes: { ...world.nodes, hubs: [] } };
}

/** This desktop's account on home as a visitor whom ada, the administrator
 *  now, asked to become a member (`world-network.md`, 一个中枢，几个账号). */
export function specimenAsVisitor(world: WorldNow): WorldNow {
  const hubs = world.accounts.hubs.map((hub) =>
    hub.hub !== HOME
      ? hub
      : {
          ...hub,
          accounts: hub.accounts.map((account) =>
            account.name === "me" ? { ...account, level: "visitor" as const, promotion: true } : account.name === "ada" ? { ...account, level: "super" as const } : account,
          ),
          shares: [],
          assists: [],
          tokens: [],
          arrivals: hub.arrivals.filter((arrival) => !arrival.code),
        },
  );
  return { ...world, accounts: { ...world.accounts, hubs } };
}

/** The same morning as the shell answers it: what `world_now` returns. */
export function specimenNow(two = false): { world: WorldNow; now: Date; lastVisit: Date; away: Away | null } {
  const { world, now, lastVisit } = specimenWorld();
  const { nodes, accounts } = hubs(two);
  // At work, the ingest service is the work account's, and so are those in it.
  if (two) {
    for (const machine of world.machines) {
      if (machine.kind !== "answered") continue;
      for (const r of machine.reply.residents) {
        if (r.town === "ingest" && r.about.machine !== "desktop") r.about.acting = { kind: "accounts", accounts: [{ hub: ACME, account: "yc" }] };
      }
    }
  }
  return {
    world: {
      here: world.here,
      who: { machine: world.here, scope: { kind: "world", towns: world.towns }, machines: [...world.machines, ...scaleWho()], others: world.others ?? [] },
      nodes: { machine: world.here, facts: facts("desktop", "windows", "C:/Users/me"), hubs: nodes },
      towns: world.towns,
      accounts: { machine: world.here, hubs: accounts },
    },
    now,
    lastVisit,
    away: world.away,
  };
}


/** What the programs on a specimen machine say of themselves: Codex from
 *  the desktop app with its models, Claude Code logged in; on this
 *  computer, a second Codex login for work, nearly spent, and a second
 *  Claude login that was logged out; on lab, Claude Code installed but not
 *  logged in; on server, Claude Code logged in but not yet let to work
 *  unasked; on render, another's, only the Claude assisting this account. */
export function specimenAgents(machine: string, at: Date = new Date()): WorldAgents {
  const efforts = (names: string[]) => names.map((effort) => ({ effort, description: "" }));
  const now = at.getTime();
  const ago = (minutes: number) => new Date(now - minutes * 60_000).toISOString();
  const later = (minutes: number) => new Date(now + minutes * 60_000).toISOString();
  const window = (name: string, minutes: number, used: number, resets: number) => ({ name, minutes, used_tenths: used, resets_at: later(resets) });
  const home = machine === "desktop" ? "C:/Users/me" : "/home/me";
  const codex: AgentFacts = {
    agent: "codex",
    home: { dir: `${home}/.codex`, found: "default" },
    account: { kind: "known", value: { email: "me@example.com", plan: "plus" } },
    limits: {
      kind: "known",
      value: { observed_at: ago(3), windows: [window("codex", 300, 423, 131), window("codex", 10080, 182, 3 * 1440 + 400)] },
    },
    program: { kind: "known", value: { path: "C:/Program Files/WindowsApps/OpenAI.Codex/app/codex.exe", source: "desktop_app", exists: true } },
    version: { kind: "known", value: "codex-cli 0.160.0" },
    launch: { kind: "known", value: { kind: "codex_app_server", program: "codex.exe", pid: 4120 } },
    login: { kind: "known", value: { kind: "logged_in", method: "ChatGPT" } },
    models: {
      kind: "known",
      value: [
        { id: "gpt-6.1-sol", model: "gpt-6.1-sol", display_name: "GPT-6.1 Sol", description: "", is_default: true, default_reasoning_effort: "medium", reasoning_efforts: efforts(["low", "medium", "high", "xhigh", "max", "ultra"]), context_window: 500_000, max_context_window: 872_000 },
        { id: "gpt-6-astra", model: "gpt-6-astra", display_name: "GPT-6 Astra", description: "", is_default: false, default_reasoning_effort: "high", reasoning_efforts: efforts(["low", "medium", "high", "xhigh", "max", "ultra"]), context_window: 272_000, max_context_window: 872_000 },
        { id: "gpt-6-luna", model: "gpt-6-luna", display_name: "GPT-6 Luna", description: "", is_default: false, default_reasoning_effort: "medium", reasoning_efforts: efforts(["low", "medium", "high", "xhigh", "max"]), context_window: 272_000, max_context_window: 872_000 },
        { id: "gpt-5.5", model: "gpt-5.5", display_name: "GPT-5.5", description: "", is_default: false, default_reasoning_effort: "medium", reasoning_efforts: efforts(["low", "medium", "high", "xhigh"]), context_window: 272_000, max_context_window: 872_000 },
      ],
    },
    integration: { kind: "known", value: "in" },
    unattended: { kind: "known", value: true },
    timings: [],
  };
  const work: AgentFacts = {
    ...codex,
    home: { dir: `${home}/.codex-work`, found: "found" },
    account: { kind: "known", value: { email: "me@acme.example", plan: "team" } },
    limits: {
      kind: "known",
      value: {
        observed_at: ago(1),
        windows: [window("codex", 300, 912, 47), window("codex", 10080, 1000, 2 * 1440 + 90)],
        credits: { balance: "52230.1460550000", unlimited: false },
      },
    },
    integration: { kind: "known", value: "in" },
  };
  const trial: AgentFacts = {
    ...codex,
    home: { dir: `${home}/.codex-trial`, found: "added" },
    account: { kind: "known", value: { email: "dev.trial@example.com", plan: "pro" } },
    limits: { kind: "known", value: { observed_at: ago(40), windows: [window("codex", 300, 96, 260), window("codex", 10080, 655, 5 * 1440)] } },
  };
  const claude: AgentFacts = {
    agent: "claude",
    home: { dir: `${home}/.claude`, found: "default" },
    account: { kind: "known", value: machine === "lab" ? {} : { email: "me@example.com", organization: "me@example.com's Organization", plan: "max" } },
    limits:
      machine === "lab"
        ? { kind: "unavailable", reason: "no session in this home has reported its limits yet" }
        : { kind: "known", value: { observed_at: ago(26), windows: [window("five_hour", 300, 618, 188), window("seven_day", 10080, 341, 4 * 1440 + 220)] } },
    program: { kind: "known", value: { path: "C:/Users/me/.local/bin/claude.exe", source: "path", exists: true } },
    version: { kind: "known", value: "2.1.289 (Claude Code)" },
    launch: { kind: "known", value: { kind: "claude_command" } },
    login: { kind: "known", value: machine === "lab" ? { kind: "logged_out" } : { kind: "logged_in", method: "claude.ai" } },
    // The World's table of Claude models (`fastctx-hosts`, claude/models.rs),
    // as a node gives it, by full model id.
    models: {
      kind: "known",
      value: [
        ["claude-opus-5-5", "Opus 5.5", "medium"],
        ["claude-fable-5-1", "Fable 5.1", "high"],
        ["claude-sonnet-5-5", "Sonnet 5.5", "medium"],
        ["claude-haiku-5-5", "Haiku 5.5", "medium"],
        ["claude-opus-5", "Opus 5", "high"],
        ["claude-sonnet-5", "Sonnet 5", "high"],
      ].map(([model, name, effort], at) => ({
        id: model!,
        model: model!,
        display_name: name!,
        description: "",
        is_default: at === 0,
        default_reasoning_effort: effort!,
        reasoning_efforts: efforts(["low", "medium", "high", "xhigh", "max"]),
        context_window: 1_000_000,
        max_context_window: 1_000_000,
      })),
    },
    integration: { kind: "known", value: "in" },
    unattended: { kind: "known", value: true },
    timings: [],
  };
  const personal: AgentFacts = {
    ...claude,
    home: { dir: `${home}/.claude-personal`, found: "added" },
    account: { kind: "unavailable", reason: "not logged in" },
    limits: { kind: "unavailable", reason: "no session in this home has reported its limits yet" },
    login: { kind: "known", value: { kind: "logged_out" } },
    integration: { kind: "known", value: "out" },
    unattended: { kind: "known", value: false },
  };
  const token: AgentFacts = {
    ...claude,
    home: { dir: `${home}/.claude-token`, found: "added" },
    account: { kind: "known", value: {} },
    login: { kind: "known", value: { kind: "logged_in", method: "oauth_token" } },
    limits: { kind: "known", value: { observed_at: ago(8), windows: [window("five_hour", 300, 935, 64), window("seven_day", 10080, 772, 2 * 1440)] } },
    unattended: { kind: "known", value: false },
  };
  // A machine of another answers only with the logins assisting the asker.
  const asks: AgentFacts = { ...claude, unattended: { kind: "known", value: false } };
  const agents =
    machine === "desktop"
      ? [codex, claude, work, trial, token, personal]
      : machine === "render"
        ? [{ ...claude, limits: { kind: "unavailable" as const, reason: "the windows of the login's account are its owner's to see" } }]
        : machine === "server"
          ? [codex, asks]
          : [codex, claude];
  return {
    machine,
    machines: [{ kind: "answered", reply: { machine, checked_at: at.toISOString(), elapsed_ms: 180, agents } }],
  };
}

/** The changes a specimen takes as asked: where other machines connect to
 *  a machine directly, so the machines page shows what follows, assistance
 *  with its deadlines, renaming, and the people and what is given to them;
 *  nothing really connects. Anything else it refuses. */
export function specimenChange(world: WorldNow | null, hub: string, made: AccountChange): WorldNow | null {
  if (!world) return null;
  // An update kept for a machine away is the hub's, not the World's.
  if (made.change === "update_due") return world;
  if (made.change === "rename") return renamed(world, hub, made.machine, made.to);
  if (made.change === "revoke_token") {
    const hubs = world.accounts.hubs.map((one) => (one.hub === hub ? { ...one, tokens: (one.tokens ?? []).filter((token) => token.key !== made.token) } : one));
    return { ...world, accounts: { ...world.accounts, hubs } };
  }
  const people = peopleChanged(world, hub, made);
  if (people) return people;
  if (made.change === "assist") {
    if (made.assisting && made.until && new Date(made.until) <= new Date()) throw new Error("the assistance's deadline has already passed");
    const home = made.home ?? null;
    const hubs = world.accounts.hubs.map((one) => {
      if (one.hub !== hub) return one;
      const kept = one.assists.filter((assist) => !(assist.machine === made.machine && assist.account === made.to && assist.agent === made.agent && assist.home === home));
      const assists = made.assisting ? [...kept, { machine: made.machine, account: made.to, agent: made.agent, home, until: made.until ?? null, ...(made.weekly ? { weekly: made.weekly } : {}), ...(made.daily ? { daily: made.daily } : {}), ...(made.five_hour ? { five_hour: made.five_hour } : {}) }] : kept;
      return { ...one, assists };
    });
    return { ...world, accounts: { ...world.accounts, hubs } };
  }
  if (made.change !== "direct") return null;
  const hubs = world.accounts.hubs.map((one) =>
    one.hub !== hub
      ? one
      : {
          ...one,
          members: one.members.map((member) => (member.name !== made.machine ? member : { ...member, direct: made.at })),
        },
  );
  return { ...world, accounts: { ...world.accounts, hubs } };
}

/** What is done to the people of `hub` and to what they are given, as the
 *  hub would do it; null for any other change. */
function peopleChanged(world: WorldNow, hub: string, made: AccountChange): WorldNow | null {
  const change = (edit: (one: HubAccounts) => HubAccounts): WorldNow => ({
    ...world,
    accounts: { ...world.accounts, hubs: world.accounts.hubs.map((one) => (one.hub === hub ? edit(one) : one)) },
  });
  switch (made.change) {
    case "open_account":
      return change((one) => {
        if (one.accounts.some((account) => account.name === made.name)) throw new Error(`the account ${made.name} already exists`);
        return { ...one, accounts: [...one.accounts, { name: made.name, level: "member", locked: false, opened: new Date().toISOString() }] };
      });
    case "set_account":
      return change((one) => ({ ...one, accounts: one.accounts.map((account) => (account.name === made.name ? { ...account, locked: made.locked } : account)) }));
    case "delete_account":
      return change((one) => ({
        ...one,
        accounts: one.accounts.filter((account) => account.name !== made.name),
        members: one.members.filter((member) => member.home !== made.name),
        shares: one.shares.filter((share) => share.account !== made.name),
        assists: one.assists.filter((assist) => assist.account !== made.name),
      }));
    case "set_password":
      return world;
    case "settings":
      return change((one) => ({ ...one, self_registration: made.self_registration }));
    case "share":
      return change((one) => ({
        ...one,
        shares: made.shared ? [...one.shares.filter((share) => !(share.machine === made.machine && share.account === made.to)), { machine: made.machine, account: made.to }] : one.shares.filter((share) => !(share.machine === made.machine && share.account === made.to)),
      }));
    case "stop_acting":
      return change((one) => ({ ...one, acting: one.acting.filter((acting) => acting.machine !== made.machine) }));
    case "seen":
      return change((one) => ({ ...one, arrivals: one.arrivals.filter((arrival) => arrival.name !== made.name) }));
    case "remove":
      specimenRemoved(made.name, made.then);
      return change((one) => ({ ...one, members: one.members.filter((member) => member.name !== made.name), arrivals: one.arrivals.filter((arrival) => arrival.name !== made.name) }));
    case "share_project":
      return change((one) => ({
        ...one,
        projects: one.projects.map((project) =>
          project.name !== made.town ? project : { ...project, granted: made.shared ? [...project.granted.filter((to) => to !== made.with), made.with] : project.granted.filter((to) => to !== made.with) },
        ),
      }));
    case "move_project":
      return change((one) => ({ ...one, projects: one.projects.map((project) => (project.name === made.town ? { ...project, owner: made.to, granted: project.granted.filter((to) => to !== made.to) } : project)) }));
    case "delete_project":
      return change((one) => ({ ...one, projects: one.projects.filter((project) => project.name !== made.town) }));
    case "relay":
      return change((one) => ({ ...one, members: one.members.map((member) => (member.name === made.machine ? { ...member, relay: made.at } : member)) }));
    case "group":
      return change((one) => ({ ...one, members: one.members.map((member) => (member.name === made.machine ? inGroup(member, made.group) : member)) }));
    case "rename_group":
      return change((one) => {
        if (!one.members.some((member) => member.home === made.owner && member.group === made.from)) throw new Error(`${made.owner} has no group ${made.from}`);
        return { ...one, members: one.members.map((member) => (member.home === made.owner && member.group === made.from ? inGroup(member, made.to) : member)) };
      });
    default:
      return null;
  }
}

/** A member in `group`, or in none. */
function inGroup(member: MemberSeen, group: string | null): MemberSeen {
  const { group: _, ...rest } = member;
  return group ? { ...rest, group } : rest;
}

/** A machine on `hub` renamed `to` as its hub would: the name it had stays
 *  as one it was, and what names it there follows. */
function renamed(world: WorldNow, hub: string, called: string, to: string): WorldNow {
  const there = world.accounts.hubs.find((one) => one.hub === hub);
  // A name it had still names it, as on the hub.
  const from = there?.members.find((member) => member.name === called || (member.was ?? []).includes(called))?.name ?? called;
  if (there?.members.some((member) => member.name !== from && (member.name === to || (member.was ?? []).includes(to)))) {
    throw new Error(`the name ${to} is taken on this hub`);
  }
  const name = (machine: string) => (machine === from ? to : machine);
  const hubs = world.accounts.hubs.map((one) =>
    one.hub !== hub
      ? one
      : {
          ...one,
          ...(one.me === from ? { me: to } : {}),
          members: one.members.map((member) => (member.name !== from ? member : { ...member, name: to, was: [...(member.was ?? []).filter((was) => was !== to), from] })),
          shares: one.shares.map((share) => ({ ...share, machine: name(share.machine) })),
          assists: one.assists.map((assist) => ({ ...assist, machine: name(assist.machine) })),
          plans: one.plans.map((plan) => ({ ...plan, machine: name(plan.machine) })),
          acting: one.acting.map((acting) => ({ ...acting, machine: name(acting.machine) })),
        },
  );
  const nodes = world.nodes.hubs.map((one) =>
    one.hub !== hub
      ? one
      : {
          ...one,
          machines: one.machines.map((node) =>
            node.name !== from ? node : { ...node, name: to, written: to, facts: "Ok" in node.facts && node.facts.Ok ? { Ok: { ...node.facts.Ok, name: to } } : node.facts },
          ),
        },
  );
  const machines = world.who.machines.map((machine) =>
    machine.kind !== "answered"
      ? { ...machine, machine: name(machine.machine) }
      : {
          ...machine,
          reply: {
            ...machine.reply,
            machine: name(machine.reply.machine),
            residents: machine.reply.residents.map((r) => ({ ...r, about: { ...r.about, machine: name(r.about.machine) } })),
            jobs: machine.reply.jobs.map((job) => ({ ...job, machine: name(job.machine) })),
          },
        },
  );
  return { ...world, accounts: { ...world.accounts, hubs }, nodes: { ...world.nodes, hubs: nodes }, who: { ...world.who, machines } };
}

/** This computer renamed `to`: on each hub it is a machine of, and here. */
export function specimenRenameHere(world: WorldNow | null, to: string): WorldNow | null {
  if (!world) return null;
  const from = world.nodes.machine;
  let next = world;
  for (const hub of world.accounts.hubs) if (hub.members.some((member) => member.name === from)) next = renamed(next, hub.hub, from, to);
  return {
    ...next,
    here: to,
    who: { ...next.who, machine: to },
    nodes: { ...next.nodes, machine: to, ...(next.nodes.facts ? { facts: { ...next.nodes.facts, name: to } } : {}) },
    accounts: { ...next.accounts, machine: to, was: [...(next.accounts.was ?? []).filter((was) => was !== to), from] },
  };
}
