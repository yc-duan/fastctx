//! What the screens say, in the user's words (`world-gui.md`, 用词): plain
//! words anyone reads at a glance. Only "World" and "Journal" carry the
//! product's character; residents are named by who they are, such as
//! "Claude · desktop"; a town is a project.
//!
//! Each message keeps its languages side by side, so one is never changed
//! without the other in view.

use fastctx_types::NameProblem;
use fastctx_types::session::HealthKind;
use fastctx_types::world::Route;
use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::sessions::Standing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

impl Lang {
    /// Its tag, as the node keeps the language chosen.
    pub fn tag(self) -> &'static str {
        match self {
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }

    /// The language a tag or a locale's name says, when it is one spoken.
    pub fn of(name: &str) -> Option<Lang> {
        let name = name.to_lowercase();
        if name.starts_with("zh") {
            Some(Lang::Zh)
        } else if name.starts_with("en") {
            Some(Lang::En)
        } else {
            None
        }
    }
}

/// The language asked for in `FASTCTX_LANG`, when it is set to one spoken:
/// asked so, no other is chosen.
pub fn asked() -> Option<Lang> {
    std::env::var("FASTCTX_LANG").ok().and_then(|name| Lang::of(&name))
}

/// The system's display language, offered first when one is chosen.
pub fn system() -> Lang {
    system_locale().and_then(|name| Lang::of(&name)).unwrap_or(Lang::En)
}

/// The first of the user's display languages. Not the locale, which sets
/// how dates and numbers are written and is often English on a machine
/// whose user reads Chinese.
#[cfg(windows)]
fn system_locale() -> Option<String> {
    use windows_sys::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    let (mut count, mut length) = (0u32, 0u32);
    // SAFETY: a null buffer asks only for the length the list needs.
    if unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, std::ptr::null_mut(), &mut length) } == 0 {
        return None;
    }
    let mut names = vec![0u16; length as usize];
    // SAFETY: the buffer is writable and holds `length` characters.
    if unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, names.as_mut_ptr(), &mut length) } == 0 {
        return None;
    }
    // A list of names, each ending in a null, the list ending in another.
    let first = names.split(|&unit| unit == 0).next()?;
    (!first.is_empty()).then(|| String::from_utf16_lossy(first))
}

#[cfg(not(windows))]
fn system_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty() && value != "C" && value != "POSIX"))
}

/// The fields of the TUI's forms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldName {
    Agent,
    Machine,
    Project,
    FirstWords,
    Slug,
    Name,
    What,
    Machines,
    Places,
    Canon,
    HubAddress,
    Account,
    Password,
    LoginName,
    HubAccount,
    DeviceKind,
    Reason,
}

/// What finishing a form does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    Open,
    Create,
    Save,
    Login,
    Device,
    Logout,
    Remove,
}

/// The words of one language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Words {
    pub lang: Lang,
}

impl Words {
    pub fn new(lang: Lang) -> Self {
        Self { lang }
    }

    fn pick(&self, zh: &'static str, en: &'static str) -> &'static str {
        match self.lang {
            Lang::Zh => zh,
            Lang::En => en,
        }
    }

    // Places.

    pub fn world(&self) -> &'static str {
        self.pick("世界", "World")
    }

    pub fn projects(&self) -> &'static str {
        self.pick("项目", "Projects")
    }

    pub fn agents(&self) -> &'static str {
        self.pick("智能体", "Agents")
    }

    pub fn outside(&self) -> &'static str {
        self.pick("其他对话", "Other conversations")
    }

    pub fn machines(&self) -> &'static str {
        self.pick("机器", "Machines")
    }

    pub fn journals(&self) -> &'static str {
        self.pick("手记", "Journals")
    }

    pub fn decisions(&self) -> &'static str {
        self.pick("定论", "Decisions")
    }

    pub fn places(&self) -> &'static str {
        self.pick("位置", "Locations")
    }

    pub fn unattended(&self) -> &'static str {
        self.pick("无人值守的作业", "Unattended jobs")
    }

    // Who is who.

    /// An agent as people call it.
    pub fn agent(&self, agent: &str) -> String {
        match agent {
            "codex" => "Codex".to_string(),
            "claude" => "Claude".to_string(),
            other => {
                let mut chars = other.chars();
                chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
            }
        }
    }

    pub fn you(&self) -> &'static str {
        self.pick("你", "You")
    }

    pub fn the_world(&self) -> &'static str {
        self.pick("世界", "The World")
    }

    pub fn opened_by_you(&self) -> &'static str {
        self.pick("由你开启", "opened by you")
    }

    /// A colleague's line: who asked for it, and the task they wrote.
    pub fn asked_by(&self, asker: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{asker} 的同事"),
            Lang::En => format!("colleague of {asker}"),
        }
    }

    pub fn this_machine(&self) -> &'static str {
        self.pick("本机", "this machine")
    }

    // How things are.

    pub fn here_busy(&self, here: usize, busy: usize) -> String {
        match (self.lang, busy) {
            (Lang::Zh, 0) => format!("{here} 个对话"),
            (Lang::Zh, _) => format!("{here} 个对话 · {busy} 个工作中"),
            (Lang::En, 0) => format!("{here} conversations"),
            (Lang::En, _) => format!("{here} conversations · {busy} working"),
        }
    }

    pub fn journal_count(&self, count: usize) -> String {
        match self.lang {
            Lang::Zh => format!("{count} 份手记"),
            Lang::En if count == 1 => "1 journal".to_string(),
            Lang::En => format!("{count} journals"),
        }
    }

    pub fn entries(&self, entries: usize, writers: usize) -> String {
        match self.lang {
            Lang::Zh => format!("{entries} 条 · {writers} 位作者"),
            Lang::En => format!(
                "{entries} {} · {writers} {}",
                if entries == 1 { "entry" } else { "entries" },
                if writers == 1 { "writer" } else { "writers" }
            ),
        }
    }

    pub fn machine_count(&self, count: usize) -> String {
        match self.lang {
            Lang::Zh => format!("{count} 台机器"),
            Lang::En if count == 1 => "1 machine".to_string(),
            Lang::En => format!("{count} machines"),
        }
    }

    pub fn busy(&self) -> &'static str {
        self.pick("工作中", "working")
    }

    pub fn waiting_for_user(&self) -> &'static str {
        self.pick("等待回复", "waiting for reply")
    }

    pub fn idle(&self) -> &'static str {
        self.pick("空闲", "idle")
    }

    pub fn asleep(&self) -> &'static str {
        self.pick("休眠", "sleeping")
    }

    pub fn left(&self) -> &'static str {
        self.pick("已离开", "left")
    }

    pub fn moved_to(&self, town: Option<&str>) -> String {
        match (self.lang, town) {
            (Lang::Zh, Some(town)) => format!("移到 {town}"),
            (Lang::Zh, None) => "移出项目".to_string(),
            (Lang::En, Some(town)) => format!("moved to {town}"),
            (Lang::En, None) => "moved out".to_string(),
        }
    }

    pub fn using_desk(&self) -> &'static str {
        self.pick("正在使用 Computer Use", "using Computer Use")
    }

    pub fn no_actions(&self) -> &'static str {
        self.pick("暂无动作", "no actions yet")
    }

    /// A job running: where, for how long, and its first line.
    pub fn running_for(&self, machine: &str, for_how_long: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{machine} · {for_how_long}"),
            Lang::En => format!("{machine} · {for_how_long}"),
        }
    }

    pub fn started_by(&self, who: &str) -> String {
        match self.lang {
            Lang::Zh => format!("由 {who} 启动"),
            Lang::En => format!("started by {who}"),
        }
    }

    pub fn online(&self) -> &'static str {
        self.pick("在线", "online")
    }

    pub fn offline(&self) -> &'static str {
        self.pick("离线", "offline")
    }

    pub fn offline_since(&self, since: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{since} 起离线"),
            Lang::En => format!("offline since {since}"),
        }
    }

    pub fn no_answer(&self) -> &'static str {
        self.pick("无响应", "no response")
    }

    pub fn alone(&self) -> &'static str {
        self.pick("未加入世界", "not in a World")
    }

    pub fn connecting(&self) -> &'static str {
        self.pick("正在连接中枢服务器", "connecting to the hub")
    }

    pub fn hub_online(&self) -> &'static str {
        self.pick("已连接中枢服务器", "connected to the hub")
    }

    pub fn hub_stopped(&self) -> &'static str {
        self.pick("已被中枢服务器移除", "removed by the hub")
    }

    pub fn no_record_here(&self, machine: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{machine} 上没有此项目的记录，已查询所有机器"),
            Lang::En => format!("no record on {machine}; asked every machine"),
        }
    }

    pub fn opened(&self, by: &str, at: &str, version: u32) -> String {
        match self.lang {
            Lang::Zh => format!("{by}创建于 {at} · 第 {version} 版"),
            Lang::En => format!("opened by {by} {at} · version {version}"),
        }
    }

    pub fn last_entry(&self, by: &str, at: &str) -> String {
        match self.lang {
            Lang::Zh => format!("最近 {by} · {at}"),
            Lang::En => format!("latest by {by}, {at}"),
        }
    }

    pub fn decisions_unread(&self, machine: &str, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("无法读取（{machine}）：{why}"),
            Lang::En => format!("couldn't read ({machine}): {why}"),
        }
    }

    pub fn no_decisions(&self) -> &'static str {
        self.pick("未设置定论位置", "no decisions folder set")
    }

    pub fn nothing_yet(&self) -> &'static str {
        self.pick("暂无", "none yet")
    }

    pub fn no_projects(&self) -> &'static str {
        self.pick("暂无项目 · p 新建", "No projects · p to create")
    }

    pub fn nobody_outside(&self) -> &'static str {
        self.pick("暂无", "none")
    }

    pub fn as_of(&self, at: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{at} 的数据"),
            Lang::En => format!("as of {at}"),
        }
    }

    pub fn asking(&self) -> &'static str {
        self.pick("正在查询…", "loading…")
    }

    pub fn node_unreachable(&self) -> &'static str {
        self.pick("无法连接本机服务", "can't reach the local service")
    }

    // Keys.

    pub fn key_open(&self) -> &'static str {
        self.pick("打开", "open")
    }

    pub fn key_talk(&self) -> &'static str {
        self.pick("对话", "talk")
    }

    pub fn key_new_agent(&self) -> &'static str {
        self.pick("新对话", "new conversation")
    }

    pub fn key_new_project(&self) -> &'static str {
        self.pick("新建项目", "new project")
    }

    /// The key that turns the workbench off, when it is `on`, or on.
    pub fn key_conversations(&self, on: bool) -> &'static str {
        if on {
            self.pick("关闭工作台", "turn the workbench off")
        } else {
            self.pick("开启工作台", "turn the workbench on")
        }
    }

    pub fn key_machines(&self) -> &'static str {
        self.pick("机器", "machines")
    }

    pub fn key_back(&self) -> &'static str {
        self.pick("返回", "back")
    }

    pub fn key_quit(&self) -> &'static str {
        self.pick("退出", "quit")
    }

    pub fn key_all(&self) -> &'static str {
        self.pick("显示休眠", "show sleeping")
    }

    // Punctuation that differs between the languages.

    /// What comes between a thing and what is said of it: "：" or ": ".
    pub fn colon(&self) -> &'static str {
        self.pick("：", ": ")
    }

    /// A remark in brackets after something.
    pub fn aside(&self, text: &str) -> String {
        match self.lang {
            Lang::Zh => format!("（{text}）"),
            Lang::En => format!(" ({text})"),
        }
    }

    /// A task label as a colleague was given it.
    pub fn quoted(&self, text: &str) -> String {
        match self.lang {
            Lang::Zh => format!("「{text}」"),
            Lang::En => format!(" “{text}”"),
        }
    }

    // More of who is who.

    pub fn here_for(&self, how_long: &str) -> String {
        match self.lang {
            Lang::Zh => format!("已加入 {how_long}"),
            Lang::En => format!("joined {how_long} ago"),
        }
    }

    pub fn nobody_here(&self) -> String {
        self.pick("暂无对话", "no conversations").to_string()
    }

    pub fn no_agents_here(&self) -> &'static str {
        self.pick("暂无对话 · n 新对话", "No conversations · n for a new one")
    }

    pub fn no_ledger_here(&self, machine: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{machine} 上没有它的记录"),
            Lang::En => format!("no record on {machine}"),
        }
    }

    pub fn heard(&self) -> &'static str {
        self.pick("收到的消息", "Messages received")
    }

    pub fn online_since(&self, since: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{since} 起在线"),
            Lang::En => format!("online since {since}"),
        }
    }

    pub fn not_asked(&self, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("无法获取：{why}"),
            Lang::En => format!("not asked: {why}"),
        }
    }

    pub fn own_sessions(&self) -> &'static str {
        self.pick("你开启的会话", "Your sessions")
    }

    /// This computer's agents, a line for each of their logins.
    pub fn agents_here(&self) -> &'static str {
        self.pick("本机的智能体", "Agents on this computer")
    }

    pub fn agent_not_installed(&self) -> &'static str {
        self.pick("未安装", "not installed")
    }

    pub fn agent_logged_in(&self) -> &'static str {
        self.pick("已登录", "logged in")
    }

    pub fn agent_logged_out(&self) -> &'static str {
        self.pick("未登录", "not signed in")
    }

    pub fn agent_login_unknown(&self) -> &'static str {
        self.pick("登录状态未知", "sign-in unknown")
    }

    /// How the World knows of a login.
    pub fn login_found(&self, found: fastctx_types::agents::HomeFound) -> &'static str {
        use fastctx_types::agents::HomeFound;
        match found {
            HomeFound::Default => self.pick("默认", "default"),
            HomeFound::Found => self.pick("自动发现", "detected"),
            HomeFound::Added => self.pick("手动添加", "added manually"),
        }
    }

    /// Whether sessions the user opens in a login have the World's tools.
    pub fn login_tools(&self, standing: fastctx_types::agents::Integration) -> &'static str {
        use fastctx_types::agents::Integration;
        match standing {
            Integration::In => self.pick("已接入", "connected"),
            Integration::Out => self.pick("未接入", "not connected"),
            Integration::Old => self.pick("fastctx 版本过旧", "fastctx outdated"),
            Integration::Absent => self.pick("未安装", "not installed"),
        }
    }

    /// The World's tools in a login, changed since they were put in.
    pub fn login_changed(&self, location: &str) -> String {
        match self.lang {
            Lang::Zh => format!("接入被改过，没有跟着更新：{location}"),
            Lang::En => format!("changed since they were put in, left as they are: {location}"),
        }
    }

    /// The World's tools in a login, waiting for the user to raise Codex's
    /// shared output limit.
    pub fn login_asks(&self, current: i64, requested: u64) -> String {
        match self.lang {
            Lang::Zh => format!("接入要更新：输出上限要从 {current} 调到 {requested}"),
            Lang::En => format!("to bring up to date: raise the output limit from {current} to {requested}"),
        }
    }

    pub fn key_take_back(&self) -> &'static str {
        self.pick("以 Fastctx 的为准", "take back")
    }

    pub fn key_update_login(&self) -> &'static str {
        self.pick("更新接入", "update")
    }

    pub fn login_needs_consent(&self) -> &'static str {
        self.pick("未授权自动运行", "not allowed to run unattended")
    }

    /// When the usage shown was read: `3 分钟前读到`.
    pub fn read_ago(&self, ago: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{ago}更新"),
            Lang::En => format!("updated {ago}"),
        }
    }

    /// What the account goes on with once a window is used up.
    pub fn credits(&self, balance: Option<&str>, unlimited: bool) -> String {
        let balance = balance.map(|balance| match balance.parse::<f64>() {
            Ok(amount) => grouped(amount.round() as i64),
            Err(_) => balance.to_string(),
        });
        match (self.lang, unlimited, balance) {
            (Lang::Zh, true, _) => "credits 不限".into(),
            (Lang::Zh, false, Some(balance)) => format!("credits 余 {balance}"),
            (Lang::Zh, false, None) => "有 credits".into(),
            (Lang::En, true, _) => "unlimited credits".into(),
            (Lang::En, false, Some(balance)) => format!("{balance} credits left"),
            (Lang::En, false, None) => "has credits".into(),
        }
    }

    pub fn limits_failed(&self, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("无法获取额度：{why}"),
            Lang::En => format!("couldn't load limits: {why}"),
        }
    }

    /// A usage window past the reset it was read before: what it holds now
    /// is not known until a session reads it again.
    pub fn limit_reset(&self) -> &'static str {
        self.pick("已重置", "reset")
    }

    /// One usage window, by its length or the agent's name for it: `5h`.
    pub fn window(&self, name: &str, minutes: Option<u32>) -> String {
        let minutes = minutes.or(match name {
            "five_hour" => Some(300),
            "seven_day" => Some(10_080),
            _ => None,
        });
        if let Some(model) = name.strip_prefix("seven_day_") {
            return format!("7d {model}");
        }
        match minutes {
            Some(m) if m % 1440 == 0 => format!("{}d", m / 1440),
            Some(m) if m % 60 == 0 => format!("{}h", m / 60),
            Some(m) => format!("{m}m"),
            None if name == "spend_limit" => self.pick("花费上限", "spend").to_string(),
            None => name.to_string(),
        }
    }

    /// How a host stands: `Codex 已接入`.
    pub fn standing(&self, host: &str, standing: Standing) -> String {
        match (self.lang, standing) {
            (Lang::Zh, Standing::In) => format!("{host} 已接入"),
            (Lang::Zh, Standing::Out) => format!("{host} 未接入"),
            (Lang::Zh, Standing::Absent) => format!("{host} 未安装"),
            (Lang::Zh, Standing::Old) => format!("{host} 中的 fastctx 版本过旧"),
            (Lang::En, Standing::In) => format!("{host}: connected"),
            (Lang::En, Standing::Out) => format!("{host}: not connected"),
            (Lang::En, Standing::Absent) => format!("{host}: not installed"),
            (Lang::En, Standing::Old) => format!("{host}: fastctx outdated"),
        }
    }

    pub fn tool_standing(&self, host: &str, standing: Standing, verified: Option<bool>) -> String {
        if standing != Standing::In {
            return self.standing(host, standing);
        }
        match (self.lang, host, verified) {
            (Lang::Zh, "Codex", Some(true)) => "Codex 已接入".into(),
            (Lang::Zh, _, Some(true)) => format!("{host} 已接入"),
            (Lang::Zh, _, _) => format!("{host} 工具待验证"),
            (Lang::En, "Codex", Some(true)) => "Codex: tools replaced".into(),
            (Lang::En, _, Some(true)) => format!("{host}: deny rules set"),
            (Lang::En, _, _) => format!("{host}: tools need checking"),
        }
    }

    pub fn tools_unverified(&self) -> &'static str {
        self.pick("配置已写入，工具验证未通过", "Configuration written; tool check failed")
    }

    /// On the World page, while an old fastctx's tools are still in Codex.
    pub fn old_tools(&self) -> &'static str {
        self.pick(
            "Codex 中的 fastctx 版本过旧 · m 机器页，u 更新",
            "Codex has an outdated fastctx · m machines, u update",
        )
    }

    pub fn applying(&self) -> &'static str {
        self.pick("接入中…", "Connecting…")
    }

    pub fn unapplying(&self) -> &'static str {
        self.pick("移除中…", "Removing…")
    }

    /// What an apply did; with `claude`, that Claude Code's open sessions
    /// lose their own shell at once, as it rereads its permissions, while the
    /// World's tools come only with a session opened again.
    pub fn applied(&self, files: usize, commands: usize, claude: bool) -> String {
        let done = match (self.lang, files + commands) {
            (Lang::Zh, 0) => "已接入，无需改动".to_string(),
            (Lang::En, 0) => "Already connected; nothing changed".to_string(),
            (Lang::Zh, _) if commands == 0 => format!("已接入：修改 {files} 个文件，新会话生效"),
            (Lang::Zh, _) => format!("已接入：修改 {files} 个文件，运行 {commands} 条 claude 命令，新会话生效"),
            (Lang::En, _) => {
                format!("Connected: {files} files changed, {commands} claude commands run; applies to new sessions")
            }
        };
        match (self.lang, claude && files + commands > 0) {
            (_, false) => done,
            (Lang::Zh, true) => {
                format!("{done}；已开着的 Claude Code 会话马上用不了自带的命令行，重新打开后才有世界的工具")
            }
            (Lang::En, true) => {
                format!(
                    "{done}; Claude Code sessions already open lose their own shell at once and get Fastctx's tools when opened again"
                )
            }
        }
    }

    pub fn unapplied(&self, files: usize, commands: usize) -> String {
        match (self.lang, files + commands) {
            (Lang::Zh, 0) => "未接入，无需改动".to_string(),
            (Lang::En, 0) => "Not connected; nothing changed".to_string(),
            (Lang::Zh, _) => format!("已移除：修改 {files} 个文件，新会话生效"),
            (Lang::En, _) => format!("Removed: {files} files changed; applies to new sessions"),
        }
    }

    pub fn needs_raise(&self, current: i64, requested: u64) -> String {
        match self.lang {
            Lang::Zh => {
                format!("Codex 输出上限 {current} 低于所需的 {requested}，未改动 · U 调高并接入（影响 Codex 所有工具）")
            }
            Lang::En => format!(
                "Codex's output limit {current} is below the {requested} needed; nothing changed · U to raise it and connect (affects all Codex tools)"
            ),
        }
    }

    /// What was done with what an old fastctx left on this machine.
    pub fn legacy_cleaned(&self, tidied: usize, archived: usize, place: &str, waiting: usize) -> String {
        match self.lang {
            Lang::Zh => {
                let mut said = format!("旧版 fastctx：已清理 {tidied} 项，归档 {archived} 项到 {place}");
                if waiting > 0 {
                    said.push_str(&format!("，{waiting} 项使用中"));
                }
                said
            }
            Lang::En => {
                let mut said = format!("Old fastctx: cleaned {tidied}, archived {archived} to {place}");
                if waiting > 0 {
                    said.push_str(&format!(", {waiting} in use"));
                }
                said
            }
        }
    }

    pub fn left_alone(&self, location: &str, reason: &str) -> String {
        match self.lang {
            Lang::Zh => format!("未改动：{location}（{reason}）"),
            Lang::En => format!("unchanged: {location} ({reason})"),
        }
    }

    pub fn key_apply(&self) -> &'static str {
        self.pick("接入", "connect")
    }

    pub fn key_replace_old(&self) -> &'static str {
        self.pick("更新", "update")
    }

    pub fn key_unapply(&self) -> &'static str {
        self.pick("移除", "remove")
    }

    /// The keys on one login: put the World's tools into it, take them out.
    pub fn key_apply_login(&self) -> &'static str {
        self.pick("接入", "connect")
    }

    pub fn key_unapply_login(&self) -> &'static str {
        self.pick("移除接入", "disconnect")
    }

    pub fn key_raise(&self) -> &'static str {
        self.pick("调高并接入", "raise and connect")
    }

    pub fn this_machine_link(&self) -> &'static str {
        self.pick("本机", "This machine")
    }

    pub fn link_alone(&self) -> String {
        self.pick(
            "未加入世界 · l 用中枢服务器地址、账号和密码登录",
            "Not in a network · l to sign in with the hub address, account and password",
        )
        .to_string()
    }

    pub fn link_connecting(&self, since: &str, error: Option<&str>) -> String {
        match (self.lang, error) {
            (Lang::Zh, Some(error)) => format!("连接中（{since} 起）：{error}"),
            (Lang::Zh, None) => format!("连接中（{since} 起）"),
            (Lang::En, Some(error)) => format!("connecting since {since}: {error}"),
            (Lang::En, None) => format!("connecting since {since}"),
        }
    }

    pub fn link_online(&self, hub: &str, through: Option<&str>, route: &Route, since: &str) -> String {
        if let Some(through) = through {
            return match (self.lang, route) {
                (Lang::Zh, Route::Local) => format!("经本机的 {through} · {since} 起"),
                (Lang::Zh, Route::Card { name }) => format!("经 {through}（{hub}）· 网卡「{name}」· {since} 起"),
                (Lang::Zh, Route::Proxy { proxy }) => format!("经 {through}（{hub}）· 代理 {proxy} · {since} 起"),
                (Lang::Zh, Route::System) => format!("经 {through}（{hub}）· {since} 起"),
                (Lang::En, Route::Local) => format!("through {through} on this machine · since {since}"),
                (Lang::En, route) => format!("through {through} ({hub}) · {route} · since {since}"),
            };
        }
        match (self.lang, route) {
            (Lang::Zh, Route::Local) => format!("已连接本机的中枢服务器 {hub} · {since} 起"),
            (Lang::Zh, Route::Card { name }) => format!("已连接 {hub} · 网卡「{name}」· {since} 起"),
            (Lang::Zh, Route::Proxy { proxy }) => format!("已连接 {hub} · 代理 {proxy} · {since} 起"),
            (Lang::Zh, Route::System) => format!("已连接 {hub} · {since} 起"),
            (Lang::En, Route::Local) => format!("connected to the hub on this machine, {hub} · since {since}"),
            (Lang::En, route) => format!("connected to {hub} · {route} · since {since}"),
        }
    }

    pub fn link_dissolved(&self) -> &'static str {
        self.pick("网络已解散", "the network was dissolved")
    }

    pub fn link_stopped(&self, reason: &str) -> String {
        match self.lang {
            Lang::Zh => format!("已被中枢服务器移除：{reason}"),
            Lang::En => format!("removed by the hub: {reason}"),
        }
    }

    // The conversation.

    pub fn say_to(&self, agent: &str) -> String {
        match self.lang {
            Lang::Zh => format!("发消息给 {agent}…"),
            Lang::En => format!("Message {agent}…"),
        }
    }

    pub fn sending(&self) -> &'static str {
        self.pick("发送中…", "sending…")
    }

    /// Words that did not go: the kernel's reason is not the interface's to
    /// show (`AGENTS.md`, 文案).
    pub fn not_sent(&self) -> &'static str {
        self.pick("发送失败", "couldn't send")
    }

    pub fn joined_turn(&self) -> &'static str {
        self.pick("已并入当前轮", "added to the current turn")
    }

    pub fn handed(&self) -> &'static str {
        self.pick("已送达", "delivered")
    }

    pub fn landing_unknown(&self) -> &'static str {
        self.pick("送达状态未知", "delivery unknown")
    }

    pub fn sent_to(&self, machine: &str) -> String {
        match self.lang {
            Lang::Zh => format!("已发往 {machine}，送达后投递"),
            Lang::En => format!("sent to {machine}; delivered when it arrives"),
        }
    }

    pub fn waiting_to_deliver(&self) -> &'static str {
        self.pick("等待投递", "waiting to deliver")
    }

    pub fn waiting_for_call(&self) -> &'static str {
        self.pick("将在它下次调用 fastctx 时送达", "delivered at its next fastctx call")
    }

    pub fn no_turn_end_known(&self) -> &'static str {
        self.pick("（宿主不报告一轮何时结束）", " (its host doesn't report turn ends)")
    }

    /// How long a tool took that ended within its first second.
    pub fn under_a_second(&self) -> &'static str {
        self.pick("不到 1 秒", "under 1s")
    }

    /// A tool that failed, after how long when that is known.
    pub fn tool_failed(&self, took: Option<&str>) -> String {
        match (self.lang, took) {
            (Lang::Zh, Some(took)) => format!("失败 · {took}"),
            (Lang::En, Some(took)) => format!("failed · {took}"),
            (Lang::Zh, None) => "失败".to_string(),
            (Lang::En, None) => "failed".to_string(),
        }
    }

    pub fn tool_declined(&self) -> &'static str {
        self.pick("已拒绝", "denied")
    }

    /// A command that exited with a code other than 0, and how long it took.
    /// A command's exit code, after how long when that is known.
    pub fn exited(&self, code: i64, took: Option<&str>) -> String {
        match (self.lang, took) {
            (Lang::Zh, Some(took)) => format!("退出码 {code} · {took}"),
            (Lang::En, Some(took)) => format!("exit {code} · {took}"),
            (Lang::Zh, None) => format!("退出码 {code}"),
            (Lang::En, None) => format!("exit {code}"),
        }
    }

    /// A command still running when its call returned, going on as `job`.
    pub fn went_on_as(&self, job: &str) -> String {
        match self.lang {
            Lang::Zh => format!("转入后台 {job}"),
            Lang::En => format!("in the background as {job}"),
        }
    }

    pub fn in_directory(&self, cwd: &str) -> String {
        match self.lang {
            Lang::Zh => cwd.to_string(),
            Lang::En => cwd.to_string(),
        }
    }

    pub fn more_lines_before(&self, lines: usize) -> String {
        match self.lang {
            Lang::Zh => format!("…上方 {lines} 行"),
            Lang::En => format!("… {lines} more lines before"),
        }
    }

    pub fn more_lines_after(&self, lines: usize) -> String {
        match self.lang {
            Lang::Zh => format!("…另有 {lines} 行"),
            Lang::En => format!("… {lines} more lines"),
        }
    }

    pub fn image(&self, mime: &str) -> String {
        match self.lang {
            Lang::Zh => format!("[图片 {mime}]"),
            Lang::En => format!("[image {mime}]"),
        }
    }

    /// A block of a tool's answer the World does not read, by its type.
    pub fn block(&self, kind: &str) -> String {
        format!("[{kind}]")
    }

    /// The key that shows or hides each tool's input and output.
    pub fn key_details(&self, shown: bool) -> &'static str {
        if shown { self.pick("收起工具", "fold tools") } else { self.pick("展开工具", "unfold tools") }
    }

    /// How much of its context a session holds: a share of its window
    /// when the host says how large that is, else a count of tokens.
    pub fn context_used(&self, used: u64, window: Option<u64>) -> String {
        let count = if used >= 1000 { format!("{:.1}k", used as f64 / 1000.0) } else { used.to_string() };
        match (self.lang, window) {
            (Lang::Zh, Some(window)) if window > 0 => format!("上下文 {}%", used * 100 / window),
            (Lang::En, Some(window)) if window > 0 => format!("context {}%", used * 100 / window),
            (Lang::Zh, _) => format!("上下文 {count} token"),
            (Lang::En, _) => format!("context {count} tokens"),
        }
    }

    /// Thinking under way, as both hosts say it, in every language.
    pub fn thinking(&self) -> &'static str {
        "Thinking…"
    }

    /// Thinking that is over, its length not told.
    pub fn thought(&self) -> &'static str {
        "Thought"
    }

    /// Thinking that is over: `Thought for 4s`, `Thought for 1m 4s`; never
    /// under a second.
    pub fn thought_for(&self, ms: u64) -> String {
        let seconds = ms.div_ceil(1000).max(1);
        let span = match seconds {
            0..60 => format!("{seconds}s"),
            60..3600 if seconds.is_multiple_of(60) => format!("{}m", seconds / 60),
            60..3600 => format!("{}m {}s", seconds / 60, seconds % 60),
            _ if (seconds / 60).is_multiple_of(60) => format!("{}h", seconds / 3600),
            _ => format!("{}h {}m", seconds / 3600, seconds / 60 % 60),
        };
        format!("Thought for {span}")
    }

    pub fn plan(&self) -> &'static str {
        self.pick("计划", "Plan")
    }

    pub fn steps(&self) -> &'static str {
        self.pick("步骤", "Steps")
    }

    pub fn compacted(&self) -> &'static str {
        self.pick("—— 上下文已压缩 ——", "—— context compacted ——")
    }

    pub fn turn_done(&self) -> &'static str {
        self.pick("本轮完成", "turn done")
    }

    pub fn turn_stopped(&self) -> &'static str {
        self.pick("本轮已停止", "turn stopped")
    }

    pub fn tool_cut(&self) -> &'static str {
        self.pick("本轮结束时仍在运行", "still running when the turn ended")
    }

    pub fn turn_failed(&self, reason: Option<&str>) -> String {
        match (self.lang, reason) {
            (Lang::Zh, Some(reason)) => format!("本轮失败：{reason}"),
            (Lang::Zh, None) => "本轮失败".to_string(),
            (Lang::En, Some(reason)) => format!("turn failed: {reason}"),
            (Lang::En, None) => "turn failed".to_string(),
        }
    }

    pub fn turn_interrupted(&self, reason: Option<&str>) -> String {
        match (self.lang, reason) {
            (Lang::Zh, Some(reason)) => format!("本轮中断：{reason}"),
            (Lang::Zh, None) => "本轮中断".to_string(),
            (Lang::En, Some(reason)) => format!("turn interrupted: {reason}"),
            (Lang::En, None) => "turn interrupted".to_string(),
        }
    }

    pub fn turn_ended_as(&self, how: &str) -> String {
        match self.lang {
            Lang::Zh => format!("本轮结束：{how}"),
            Lang::En => format!("turn ended: {how}"),
        }
    }

    /// What kind of fact about a session a health line reports; the host's
    /// own words follow it.
    pub fn health(&self, kind: HealthKind) -> &'static str {
        match kind {
            HealthKind::Reconnecting => self.pick("正在重连模型", "reconnecting to the model"),
            HealthKind::Safety => self.pick("服务商安全检查中", "provider safety check"),
            HealthKind::Rerouted => self.pick("服务商已切换模型", "provider switched model"),
            HealthKind::ToolsMissing => self.pick("会话未接入 fastctx", "fastctx not connected in this session"),
            HealthKind::ToolServer => self.pick("其他工具服务启动失败", "another tool server failed to start"),
            HealthKind::Warning => self.pick("宿主警告", "host warning"),
            HealthKind::Error => self.pick("宿主错误", "host error"),
        }
    }

    pub fn stopping(&self) -> &'static str {
        self.pick("已请求停止", "stop requested")
    }

    pub fn no_turn_running(&self) -> &'static str {
        self.pick("当前没有运行", "not running")
    }

    pub fn not_stopped(&self) -> &'static str {
        self.pick("停止失败", "couldn't stop")
    }

    /// A conversation whose last turn went wrong, in the list of who works.
    pub fn troubled(&self) -> &'static str {
        self.pick("上一轮没有完成", "last turn did not finish")
    }

    pub fn opening_session(&self) -> &'static str {
        self.pick("正在打开…", "opening…")
    }

    pub fn nothing_said_yet(&self) -> &'static str {
        self.pick("暂无内容", "Nothing to show yet")
    }

    pub fn more_below(&self) -> &'static str {
        self.pick("更多 ↓", "more ↓")
    }

    /// Rows below a conversation that did not fit.
    pub fn more_rows(&self, count: usize) -> String {
        match self.lang {
            Lang::Zh => format!("另有 {count} 项"),
            Lang::En => format!("{count} more"),
        }
    }

    pub fn pasting(&self, count: usize) -> String {
        match self.lang {
            Lang::Zh => format!("正在粘贴（{count} 字）…"),
            Lang::En => format!("pasting ({count} characters)…"),
        }
    }

    pub fn watch_broke(&self, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("连接中断：{why}"),
            Lang::En => format!("disconnected: {why}"),
        }
    }

    pub fn watch_ended(&self) -> &'static str {
        self.pick("已停止同步", "sync stopped")
    }

    pub fn watch_interrupted(&self, lasted: &str, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("连接中断，正在重连…（{lasted}：{why}）"),
            Lang::En => format!("connection lost, reconnecting… ({lasted}: {why})"),
        }
    }

    // Journals.

    pub fn described(&self, by: &str, at: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{by} · {at}"),
            Lang::En => format!("described by {by} {at}"),
        }
    }

    pub fn earlier_entries(&self, count: usize) -> String {
        match self.lang {
            Lang::Zh => format!("更早 {count} 条 · [ 向前"),
            Lang::En => format!("{count} earlier · [ to go back"),
        }
    }

    pub fn entries_shown(&self, first: usize, last: usize, total: usize) -> String {
        match self.lang {
            Lang::Zh => format!("第 {first}–{last} 条，共 {total} 条"),
            Lang::En => format!("entries {first}–{last} of {total}"),
        }
    }

    pub fn reading(&self) -> &'static str {
        self.pick("读取中…", "reading…")
    }

    pub fn self_reported(&self, model: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{model}（自报）"),
            Lang::En => format!("{model} (self-reported)"),
        }
    }

    pub fn answers(&self, ids: &str) -> String {
        match self.lang {
            Lang::Zh => format!("回应 {ids}"),
            Lang::En => format!("answers {ids}"),
        }
    }

    pub fn mentioned_by(&self, ids: &str) -> String {
        match self.lang {
            Lang::Zh => format!("被引用：{ids}"),
            Lang::En => format!("mentioned by {ids}"),
        }
    }

    pub fn attachments(&self, listed: &str) -> String {
        match self.lang {
            Lang::Zh => format!("附件：{listed}"),
            Lang::En => format!("Attachments: {listed}"),
        }
    }

    /// One attachment: its name, kind and size, and the machine keeping it
    /// when it is too large for the ledger.
    pub fn attachment(&self, name: &str, kind: &str, size: &str, kept_on: Option<&str>) -> String {
        match (self.lang, kept_on) {
            (Lang::Zh, Some(machine)) => format!("{name}（{kind}，{size}，留在 {machine} 上）"),
            (Lang::Zh, None) => format!("{name}（{kind}，{size}）"),
            (Lang::En, Some(machine)) => format!("{name} ({kind}, {size}, kept on {machine})"),
            (Lang::En, None) => format!("{name} ({kind}, {size})"),
        }
    }

    pub fn kind_text(&self) -> &'static str {
        self.pick("文本", "text")
    }

    pub fn kind_image(&self) -> &'static str {
        self.pick("图片", "image")
    }

    pub fn kind_binary(&self) -> &'static str {
        self.pick("二进制", "binary")
    }

    // Forms.

    pub fn no_project(&self) -> &'static str {
        self.pick("无", "none")
    }

    pub fn form_open_agent(&self) -> &'static str {
        self.pick("新对话", "New conversation")
    }

    pub fn form_new_project(&self) -> &'static str {
        self.pick("新建项目", "New project")
    }

    pub fn form_edit_project(&self, town: &str) -> String {
        match self.lang {
            Lang::Zh => format!("编辑 {town}"),
            Lang::En => format!("Edit {town}"),
        }
    }

    pub fn form_login(&self) -> &'static str {
        self.pick("登录", "Sign in")
    }

    pub fn form_device(&self) -> &'static str {
        self.pick("添加设备", "Add device")
    }

    pub fn form_logout(&self) -> &'static str {
        self.pick("退出账号", "Sign out")
    }

    pub fn form_remove(&self, name: &str, account: &str) -> String {
        match self.lang {
            Lang::Zh => format!("从 {account} 移除 {name}"),
            Lang::En => format!("Remove {name} from {account}"),
        }
    }

    pub fn remove_warning(&self, name: &str, account: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{name} 将退出 {account}；若是它的机器，还会断开中枢服务器，需重新登录才能回来。"),
            Lang::En => format!(
                "{name} will be signed out of {account}; a machine of the account also disconnects from the hub until it signs in again."
            ),
        }
    }

    pub fn logout_warning(&self) -> &'static str {
        self.pick(
            "这台电脑将看不到该账号的机器、对话与项目；若它是该账号的机器，还会断开中枢服务器。",
            "This computer will no longer see the account's machines, conversations and projects; if it is the account's machine, it also disconnects from the hub.",
        )
    }

    pub fn device_computer(&self) -> &'static str {
        self.pick("电脑", "Computer")
    }

    /// A Linux host, with how long the line that installs it works.
    pub fn device_server(&self, hours: u32) -> String {
        let valid = self.duration(u64::from(hours) * 3_600);
        match self.lang {
            Lang::Zh => format!("Linux 主机（{valid}内有效）"),
            Lang::En => format!("Linux host (valid {valid})"),
        }
    }

    pub fn computer_how(&self) -> &'static str {
        self.pick("在那台电脑上用这个账号和密码登录", "Sign in on that computer with this account and its password")
    }

    pub fn server_how(&self) -> &'static str {
        self.pick("在 Linux 主机上运行：", "Run on the Linux host:")
    }

    pub fn server_until(&self, until: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{until} 前有效，仅可使用一次"),
            Lang::En => format!("Valid until {until}, single use"),
        }
    }

    pub fn logged_in(&self, hub: &str, name: &str, account: &str) -> String {
        match self.lang {
            Lang::Zh => format!("已登录 {hub} · {account} · 本机名 {name}"),
            Lang::En => format!("Signed in to {hub} · {account} · this computer is {name}"),
        }
    }

    pub fn login_needs(&self) -> &'static str {
        self.pick("请填写中枢服务器地址、账号和密码", "Enter the hub address, account and password")
    }

    pub fn hub_section(&self, hub: &str) -> String {
        match self.lang {
            Lang::Zh => format!("中枢服务器 {hub}"),
            Lang::En => format!("Hub {hub}"),
        }
    }

    pub fn logins_line(&self, logins: &str, home: Option<&str>) -> String {
        match (self.lang, home) {
            (Lang::Zh, Some(home)) => format!("已登录 {logins} · {home} 的机器"),
            (Lang::Zh, None) => format!("已登录 {logins}"),
            (Lang::En, Some(home)) => format!("signed in to {logins} · {home}'s machine"),
            (Lang::En, None) => format!("signed in to {logins}"),
        }
    }

    pub fn level_super(&self) -> &'static str {
        self.pick("管理员", "administrator")
    }

    pub fn level_visitor(&self) -> &'static str {
        self.pick("访客", "visitor")
    }

    /// After a member's logins: who administers the hub, and so may use
    /// every machine of theirs.
    pub fn admin_is(&self, admin: &str) -> String {
        match self.lang {
            Lang::Zh => format!(" · 管理员 {admin}（可访问你的所有机器）"),
            Lang::En => format!(" · administrator {admin} (can access all your machines)"),
        }
    }

    /// After a visitor's logins: who administers the hub, who only sees
    /// which machines there are (`world-network.md`, 一个中枢，几个账号).
    pub fn admin_sees(&self, admin: &str) -> String {
        match self.lang {
            Lang::Zh => format!(" · 管理员 {admin}（只看得到你有哪几台机器）"),
            Lang::En => format!(" · administrator {admin} (only sees which machines you have)"),
        }
    }

    /// A managed machine's one screen (`world-gui.md`, 受管机器上的界面).
    pub fn managed_title(&self) -> &'static str {
        self.pick("受管机器", "Managed machine")
    }

    pub fn managed_whose(&self, owner: &str, machine: &str, hub: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{owner} 的 {machine} · 在 {hub}"),
            Lang::En => format!("{owner}'s {machine} · on {hub}"),
        }
    }

    pub fn managed_online(&self, ms: Option<u32>) -> String {
        match (self.lang, ms) {
            (Lang::Zh, Some(ms)) => format!("已连接 · {ms} 毫秒"),
            (Lang::Zh, None) => "已连接".to_string(),
            (Lang::En, Some(ms)) => format!("Connected · {ms} ms"),
            (Lang::En, None) => "Connected".to_string(),
        }
    }

    pub fn managed_away(&self, since: &str) -> String {
        match self.lang {
            Lang::Zh => format!("断开 · 从 {since} 起"),
            Lang::En => format!("Disconnected since {since}"),
        }
    }

    pub fn managed_removed(&self) -> &'static str {
        self.pick("已被移出网络", "Taken off the network")
    }

    /// Over the machines of one group on a hub (`world-network.md`, 机器的分组):
    /// its name, and whose it is where it is not this computer's accounts'.
    pub fn group_head(&self, owner: Option<&str>, group: &str) -> String {
        match (self.lang, owner) {
            (Lang::Zh, Some(owner)) => format!("{owner} 的分组 · {group}"),
            (Lang::Zh, None) => format!("分组 · {group}"),
            (Lang::En, Some(owner)) => format!("{owner}'s group · {group}"),
            (Lang::En, None) => format!("group · {group}"),
        }
    }

    pub fn owner_and_rights(&self, owner: Option<&str>, rights: &str) -> String {
        match (self.lang, owner) {
            (Lang::Zh, Some(owner)) => format!("{owner} 的机器  ·  {rights}"),
            (Lang::Zh, None) => rights.to_string(),
            (Lang::En, Some(owner)) => format!("{owner}'s machine  ·  {rights}"),
            (Lang::En, None) => rights.to_string(),
        }
    }

    /// A choice of an agent assisting this computer's accounts: the machine
    /// it lives on, whose that is, and which agent.
    pub fn assisting_choice(&self, machine: &str, owner: Option<&str>, agent: &str) -> String {
        let agent = if agent == "claude" {
            "Claude"
        } else if agent == "codex" {
            "Codex"
        } else {
            agent
        };
        match (self.lang, owner) {
            (Lang::Zh, Some(owner)) => format!("{agent} · {machine}（来自 {owner} 的协助）"),
            (Lang::Zh, None) => format!("{agent} · {machine}（协助）"),
            (Lang::En, Some(owner)) => format!("{agent} on {machine} (assisting, from {owner})"),
            (Lang::En, None) => format!("{agent} on {machine} (assisting)"),
        }
    }

    /// Whether `account` may use a machine: work on it and use every agent
    /// on it, which go together (`world-network.md`, 共享机器与借出智能体).
    pub fn rights(&self, account: &str, hands: bool) -> String {
        match (self.lang, hands) {
            (Lang::Zh, true) => format!("{account} 可用"),
            (Lang::Zh, false) => format!("{account} 不可用"),
            (Lang::En, true) => format!("available to {account}"),
            (Lang::En, false) => format!("not available to {account}"),
        }
    }

    pub fn arrivals(&self) -> &'static str {
        self.pick("新设备", "New devices")
    }

    pub fn arrival(&self, name: &str, device: bool, account: &str, hub: &str, at: &str) -> String {
        match (self.lang, device) {
            (Lang::Zh, true) => format!("{at} · 设备 {name} 登录 {hub} · {account}"),
            (Lang::Zh, false) => format!("{at} · 机器 {name} 加入 {hub} · {account}"),
            (Lang::En, true) => format!("{at} · device {name} signed in to {hub} · {account}"),
            (Lang::En, false) => format!("{at} · machine {name} joined {hub} · {account}"),
        }
    }

    pub fn key_login(&self) -> &'static str {
        self.pick("登录", "sign in")
    }

    pub fn key_device(&self) -> &'static str {
        self.pick("添加设备", "add device")
    }

    pub fn key_logout(&self) -> &'static str {
        self.pick("退出账号", "sign out")
    }

    pub fn key_remove(&self) -> &'static str {
        self.pick("移除", "remove")
    }

    pub fn not_me_reason(&self) -> &'static str {
        self.pick("用户确认不是本人", "the user said it was not them")
    }

    /// A network this computer was on, dissolved by its administrator.
    pub fn dissolved(&self, hub: &str, at: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{at} · {hub} 已被管理员解散，这台电脑已离开；Fastctx 和这里的数据都还在"),
            Lang::En => format!(
                "{at} · {hub} was dissolved by its administrator; this computer has left, and Fastctx and your data here are untouched"
            ),
        }
    }

    pub fn key_seen_dissolved(&self) -> &'static str {
        self.pick("知道了", "got it")
    }

    pub fn key_its_me(&self) -> &'static str {
        self.pick("是我", "it was me")
    }

    pub fn key_not_me(&self) -> &'static str {
        self.pick("不是我", "not me")
    }

    /// What came of keeping this machine connected after it joined.
    pub fn kept(&self, kept: &fastctx_install::service::Kept) -> String {
        use fastctx_install::service::Kept;
        match (self.lang, kept) {
            (Lang::Zh, Kept::Already | Kept::Installed) => "开机自启已开启，断网后自动重连".to_string(),
            (Lang::En, Kept::Already | Kept::Installed) => "Starts at boot and reconnects automatically".to_string(),
            (Lang::Zh, Kept::Declined) => "开机自启未开启 · 以管理员运行 fastctx service node install".to_string(),
            (Lang::En, Kept::Declined) => {
                "Not starting at boot · run fastctx service node install as administrator".to_string()
            }
            (Lang::Zh, Kept::Unsupported) => "此系统不支持开机自启".to_string(),
            (Lang::En, Kept::Unsupported) => "Starting at boot isn't supported on this system".to_string(),
            (Lang::Zh, Kept::Command(command)) => format!("开机自启需运行：{command}"),
            (Lang::En, Kept::Command(command)) => format!("To start at boot, run: {command}"),
        }
    }

    pub fn not_kept(&self, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("开机自启未开启：{why}"),
            Lang::En => format!("Couldn't set up start at boot: {why}"),
        }
    }

    pub fn working(&self) -> &'static str {
        self.pick("处理中…", "working…")
    }

    pub fn say_something_first(&self) -> &'static str {
        self.pick("请输入第一条消息", "Enter the first message")
    }

    pub fn name_and_what_needed(&self) -> &'static str {
        self.pick("请填写名称和须知", "Enter a name and the instructions")
    }

    /// What is wrong with a machine or server name as typed, in one sentence
    /// (`fastctx_types::NameProblem`).
    pub fn name_problem(&self, problem: &NameProblem) -> String {
        // Nothing to show between the brackets: say it is invisible.
        let invisible =
            |character: char| unicode_width::UnicodeWidthChar::width(character).is_none_or(|width| width == 0);
        match (self.lang, problem) {
            (Lang::Zh, NameProblem::Empty) => "请填写名字".to_string(),
            (Lang::En, NameProblem::Empty) => "Enter a name".to_string(),
            (Lang::Zh, NameProblem::TooLong) => "名字太长：最多 32 个字，中文最多 21 个".to_string(),
            (Lang::En, NameProblem::TooLong) => {
                "That name is too long: 32 characters at most, 21 in Chinese".to_string()
            }
            (Lang::Zh, NameProblem::Space) => "名字里不能有空格".to_string(),
            (Lang::En, NameProblem::Space) => "A name can't contain spaces".to_string(),
            (Lang::Zh, NameProblem::NotAllowed { character }) if invisible(*character) => {
                "名字里有看不见的字符".to_string()
            }
            (Lang::En, NameProblem::NotAllowed { character }) if invisible(*character) => {
                "That name contains an invisible character".to_string()
            }
            (Lang::Zh, NameProblem::NotAllowed { character }) => {
                format!("名字里不能有「{character}」，只能用字母、数字和连字符")
            }
            (Lang::En, NameProblem::NotAllowed { character }) => {
                format!("A name can't contain \u{201c}{character}\u{201d}: use letters, digits and hyphens")
            }
            (Lang::Zh, NameProblem::BadStart { character: '-' }) => "名字不能以连字符开头".to_string(),
            (Lang::En, NameProblem::BadStart { character: '-' }) => "A name can't start with a hyphen".to_string(),
            (Lang::Zh, NameProblem::BadStart { character }) => format!("名字不能以「{character}」开头"),
            (Lang::En, NameProblem::BadStart { character }) => {
                format!("A name can't start with \u{201c}{character}\u{201d}")
            }
            (Lang::Zh, NameProblem::EndsWithHyphen) => "名字不能以连字符结尾".to_string(),
            (Lang::En, NameProblem::EndsWithHyphen) => "A name can't end with a hyphen".to_string(),
            (Lang::Zh, NameProblem::MixedScripts { first, second }) => {
                format!("「{first}」和「{second}」是两种容易看混的文字，不能放在一个名字里")
            }
            (Lang::En, NameProblem::MixedScripts { first, second }) => format!(
                "\u{201c}{first}\u{201d} and \u{201c}{second}\u{201d} are from scripts that can't be mixed in one name"
            ),
            (Lang::Zh, NameProblem::Reserved) => "这个名字留给中枢服务器自己用".to_string(),
            (Lang::En, NameProblem::Reserved) => "That name is reserved for the hub".to_string(),
        }
    }

    pub fn place_wrong(&self, line: &str) -> String {
        match self.lang {
            Lang::Zh => format!("格式应为「名称 | 机器 | 路径」：{line}"),
            Lang::En => format!("expected \"name | machine | path\": {line}"),
        }
    }

    pub fn canon_wrong(&self, canon: &str) -> String {
        match self.lang {
            Lang::Zh => format!("格式应为「机器 | 路径」：{canon}"),
            Lang::En => format!("expected \"machine | path\": {canon}"),
        }
    }

    pub fn field(&self, field: FieldName) -> &'static str {
        match field {
            FieldName::Agent => self.pick("智能体", "Agent"),
            FieldName::Machine => self.pick("机器", "Machine"),
            FieldName::Project => self.pick("项目", "Project"),
            FieldName::FirstWords => self.pick("第一条消息", "First message"),
            FieldName::Slug => self.pick("短名", "Short name"),
            FieldName::Name => self.pick("名称", "Name"),
            FieldName::What => self.pick("须知", "Instructions"),
            FieldName::Machines => self.pick("机器", "Machines"),
            FieldName::Places => self.pick("位置", "Locations"),
            FieldName::Canon => self.pick("定论", "Decisions"),
            FieldName::HubAddress => self.pick("中枢服务器地址", "Hub address"),
            FieldName::Account => self.pick("账号", "Account"),
            FieldName::Password => self.pick("密码", "Password"),
            FieldName::LoginName => self.pick("本机名称", "Computer name"),
            FieldName::HubAccount => self.pick("账号", "Account"),
            FieldName::DeviceKind => self.pick("设备", "Device"),
            FieldName::Reason => self.pick("原因", "Reason"),
        }
    }

    pub fn hint(&self, field: FieldName) -> Option<&'static str> {
        Some(match field {
            FieldName::FirstWords => self.pick("Ctrl+J 换行", "Ctrl+J for a new line"),
            FieldName::Slug => self.pick("例如 mark50", "e.g. mark50"),
            FieldName::Machines => self.pick("← → 移动 · Space 勾选", "← → move · Space tick"),
            FieldName::Places => self.pick("名称 | 机器 | 路径，每行一项", "name | machine | path, one per line"),
            FieldName::Canon => self.pick("机器 | 路径", "machine | path"),
            FieldName::HubAddress => self.pick("例如 hub.example.com:7443", "e.g. hub.example.com:7443"),
            FieldName::DeviceKind => self.pick("← → 选择", "← → choose"),
            FieldName::LoginName => self.pick("留空则用主机名", "empty uses the host name"),
            _ => return None,
        })
    }

    // More keys.

    pub fn key_present_only(&self) -> &'static str {
        self.pick("隐藏休眠", "hide sleeping")
    }

    pub fn key_read(&self) -> &'static str {
        self.pick("阅读", "read")
    }

    pub fn key_new_agent_here(&self) -> &'static str {
        self.pick("新对话", "new conversation")
    }

    pub fn key_edit_project(&self) -> &'static str {
        self.pick("编辑项目", "edit project")
    }

    pub fn key_send(&self) -> &'static str {
        self.pick("发送", "send")
    }

    pub fn key_newline(&self) -> &'static str {
        self.pick("换行", "new line")
    }

    pub fn key_scroll(&self) -> &'static str {
        self.pick("翻看", "scroll")
    }

    pub fn key_stop(&self) -> &'static str {
        self.pick("停止", "stop")
    }

    pub fn key_watch_again(&self) -> &'static str {
        self.pick("重新连接", "reconnect")
    }

    pub fn key_attach(&self) -> &'static str {
        self.pick("在原生界面中打开", "open in its own interface")
    }

    pub fn attach_elsewhere(&self, machine: &str) -> String {
        match self.lang {
            Lang::Zh => format!("请在 {machine} 上打开"),
            Lang::En => format!("open it on {machine}"),
        }
    }

    pub fn attach_failed(&self, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("打开失败：{why}"),
            Lang::En => format!("couldn't open: {why}"),
        }
    }

    pub fn attach_exited(&self, code: Option<i32>) -> String {
        match (self.lang, code) {
            (Lang::Zh, Some(code)) => format!("原生界面已退出（{code}）"),
            (Lang::Zh, None) => "原生界面已中止".to_string(),
            (Lang::En, Some(code)) => format!("its own interface exited with {code}"),
            (Lang::En, None) => "its own interface was ended".to_string(),
        }
    }

    pub fn key_colleagues(&self) -> &'static str {
        self.pick("同事", "colleagues")
    }

    pub fn key_open_colleague(&self) -> &'static str {
        self.pick("打开同事对话", "open colleague")
    }

    pub fn key_back_to_typing(&self) -> &'static str {
        self.pick("回到输入", "back to typing")
    }

    pub fn key_earlier(&self) -> &'static str {
        self.pick("更早", "earlier")
    }

    pub fn key_confirm(&self) -> &'static str {
        self.pick("确认", "confirm")
    }

    pub fn key_cancel(&self) -> &'static str {
        self.pick("取消", "cancel")
    }

    pub fn key_next_field(&self) -> &'static str {
        self.pick("下一项", "next")
    }

    pub fn key_choose(&self) -> &'static str {
        self.pick("选择", "choose")
    }

    pub fn key_tick(&self) -> &'static str {
        self.pick("勾选", "tick")
    }

    /// What Enter does on a form's last field.
    pub fn key_finish(&self, finish: &Finish) -> &'static str {
        match finish {
            Finish::Open => self.pick("开始", "start"),
            Finish::Create => self.pick("创建", "create"),
            Finish::Save => self.pick("保存", "save"),
            Finish::Login => self.pick("登录", "sign in"),
            Finish::Device => self.pick("生成", "generate"),
            Finish::Logout => self.pick("退出", "sign out"),
            Finish::Remove => self.pick("移除", "remove"),
        }
    }

    // Time.

    /// How long, as `40 秒`, `3 分钟`, `2 小时 10 分`, `4 天 2 小时`.
    pub fn duration(&self, seconds: u64) -> String {
        let (days, hours, minutes) = (seconds / 86_400, seconds / 3_600 % 24, seconds / 60 % 60);
        match self.lang {
            Lang::Zh => match (days, hours, minutes) {
                (0, 0, 0) => format!("{seconds} 秒"),
                (0, 0, minutes) => format!("{minutes} 分钟"),
                (0, hours, 0) => format!("{hours} 小时"),
                (0, hours, minutes) => format!("{hours} 小时 {minutes} 分"),
                (days, 0, _) => format!("{days} 天"),
                (days, hours, _) => format!("{days} 天 {hours} 小时"),
            },
            Lang::En => match (days, hours, minutes) {
                (0, 0, 0) => format!("{seconds} s"),
                (0, 0, minutes) => format!("{minutes} min"),
                (0, hours, 0) => format!("{hours} h"),
                (0, hours, minutes) => format!("{hours} h {minutes} min"),
                (days, 0, _) => format!("{days} d"),
                (days, hours, _) => format!("{days} d {hours} h"),
            },
        }
    }

    pub fn running_unseen(&self, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("无法获取同事与作业：{why}"),
            Lang::En => format!("couldn't load colleagues and jobs: {why}"),
        }
    }

    pub fn since(&self, then: Timestamp, now: Timestamp) -> String {
        self.duration(seconds_between(then, now))
    }

    /// How long ago: `刚刚`, `3 分钟前`.
    pub fn ago(&self, then: Timestamp, now: Timestamp) -> String {
        let seconds = seconds_between(then, now);
        match self.lang {
            Lang::Zh if seconds < 60 => "刚刚".to_string(),
            Lang::En if seconds < 60 => "just now".to_string(),
            Lang::Zh => format!("{}前", self.duration(seconds)),
            Lang::En => format!("{} ago", self.duration(seconds)),
        }
    }
}

/// A whole number with its thousands set apart: `52,230`.
fn grouped(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    if value < 0 { format!("-{out}") } else { out }
}

fn seconds_between(then: Timestamp, now: Timestamp) -> u64 {
    now.as_second().saturating_sub(then.as_second()).max(0) as u64
}

/// A moment as the local clock reads it: the time today, the date and time
/// on another day, the year too in another year.
pub fn clock(at: Timestamp, now: Timestamp) -> String {
    let zone = TimeZone::system();
    let (at, now) = (at.to_zoned(zone.clone()), now.to_zoned(zone));
    if at.date() == now.date() {
        at.strftime("%H:%M").to_string()
    } else if at.year() == now.year() {
        at.strftime("%m-%d %H:%M").to_string()
    } else {
        at.strftime("%Y-%m-%d").to_string()
    }
}
