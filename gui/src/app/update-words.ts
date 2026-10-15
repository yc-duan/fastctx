// The words of new versions: the sidebar's button, its card, the settings'
// rows, another machine's update, and the network's policy.

export interface UpdateWords {
  /** The sidebar's button. */
  bar: string;
  barFetching(percent: number | null): string;
  barReady: string;
  barInstalling: string;
  /** The card. */
  title: string;
  close: string;
  named(version: string): string;
  beta: string;
  size(megabytes: string): string;
  viaNetwork: string;
  go: string;
  restart: string;
  retry: string;
  fetching: string;
  waiting: string;
  installing: string;
  phoneAsks: string;
  permission: string;
  minimum(version: string): string;
  failed(why: string): string;
  /** The settings. */
  updates: string;
  current(version: string): string;
  look: string;
  looking: string;
  latest: string;
  available(version: string): string;
  unreachable: string;
  heardFromNetwork: string;
  auto: string;
  betaChoice: string;
  followNetwork: string;
  on: string;
  off: string;
  relay: string;
  /** Another machine. */
  updateMachine: string;
  machineBehind(version: string): string;
  machineUpdating: string;
  /** The network's policy. */
  policy: string;
  channel: string;
  stable: string;
  minimumField: string;
  minimumNone: string;
  minimumInvalid: string;
  save: string;
}

export const zhUpdate: UpdateWords = {
  bar: "更新",
  barFetching: (percent) => (percent === null ? "下载中" : `下载中 ${percent}%`),
  barReady: "重启以更新",
  barInstalling: "正在更新",
  title: "新版本",
  close: "关闭",
  named: (version) => `Fastctx ${version}`,
  beta: "测试版",
  size: (megabytes) => `${megabytes} MB`,
  viaNetwork: "经网络下载",
  go: "更新",
  restart: "重启并更新",
  retry: "重试",
  fetching: "下载中",
  waiting: "等手上的活告一段落",
  installing: "正在安装，应用会自己重新打开",
  phoneAsks: "系统会请你确认安装",
  permission: "在系统设置里允许 Fastctx 安装应用，回来再点更新",
  minimum: (version) => `网络要求至少 ${version}`,
  failed: (why) => `没能更新：${why}`,
  updates: "更新",
  current: (version) => `Fastctx ${version}`,
  look: "检查更新",
  looking: "正在检查…",
  latest: "已是最新",
  available: (version) => `有新版本 ${version}`,
  unreachable: "连不上 GitHub",
  heardFromNetwork: "经网络得知",
  auto: "自动更新",
  betaChoice: "测试版",
  followNetwork: "跟随网络",
  on: "开",
  off: "关",
  relay: "帮网络里的设备传更新包",
  updateMachine: "更新",
  machineBehind: (version) => `${version} 可用`,
  machineUpdating: "正在更新…",
  policy: "网络的更新",
  channel: "通道",
  stable: "正式版",
  minimumField: "最低版本",
  minimumNone: "不要求",
  minimumInvalid: "写成 1.0.1 或 1.0.1-beta.2 这样",
  save: "保存",
};

export const enUpdate: UpdateWords = {
  bar: "Update",
  barFetching: (percent) => (percent === null ? "Downloading" : `Downloading ${percent}%`),
  barReady: "Restart to update",
  barInstalling: "Updating",
  title: "New version",
  close: "Close",
  named: (version) => `Fastctx ${version}`,
  beta: "Beta",
  size: (megabytes) => `${megabytes} MB`,
  viaNetwork: "Downloaded through the network",
  go: "Update",
  restart: "Restart and update",
  retry: "Try again",
  fetching: "Downloading",
  waiting: "Waiting for the work under way",
  installing: "Installing; the app opens again by itself",
  phoneAsks: "The system will ask you to confirm",
  permission: "Allow Fastctx to install apps in the system settings, then tap Update again",
  minimum: (version) => `The network asks for at least ${version}`,
  failed: (why) => `Couldn't update: ${why}`,
  updates: "Updates",
  current: (version) => `Fastctx ${version}`,
  look: "Check for updates",
  looking: "Checking…",
  latest: "Up to date",
  available: (version) => `Version ${version} available`,
  unreachable: "Can't reach GitHub",
  heardFromNetwork: "Heard from the network",
  auto: "Automatic updates",
  betaChoice: "Beta versions",
  followNetwork: "As the network",
  on: "On",
  off: "Off",
  relay: "Pass updates to the network's devices",
  updateMachine: "Update",
  machineBehind: (version) => `${version} available`,
  machineUpdating: "Updating…",
  policy: "Network updates",
  channel: "Channel",
  stable: "Stable",
  minimumField: "Minimum version",
  minimumNone: "None",
  minimumInvalid: "Write it as 1.0.1 or 1.0.1-beta.2",
  save: "Save",
};
