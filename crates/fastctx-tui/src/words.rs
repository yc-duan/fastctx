//! What the screens say, in the user's words (`world-gui.md`, 用词): plain
//! words anyone reads at a glance. Only "World" and "Journal" carry the
//! product's character; residents are named by who they are, such as
//! "Claude · desktop"; a town is a project.
//!
//! Each message keeps its languages side by side, so one is never changed
//! without the other in view.

use std::sync::OnceLock;

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

/// The user's language: `FASTCTX_LANG` when set, otherwise the system's.
pub fn lang() -> Lang {
    static LANG: OnceLock<Lang> = OnceLock::new();
    *LANG.get_or_init(|| {
        let name = std::env::var("FASTCTX_LANG").ok().filter(|name| !name.is_empty()).or_else(system_locale);
        match name {
            Some(name) if name.to_lowercase().starts_with("zh") => Lang::Zh,
            _ => Lang::En,
        }
    })
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
    Register,
    LoginName,
    Token,
    HubAccount,
    TokenFor,
    Hours,
    Reason,
}

/// What finishing a form does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    Open,
    Create,
    Save,
    Login,
    Token,
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
        self.pick("不在任何项目里", "Not in a project")
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
        self.pick("在哪里能找到什么", "Where things are")
    }

    pub fn unattended(&self) -> &'static str {
        self.pick("还在跑，起它的已经不在", "Still running, nobody here who started it")
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
        self.pick("你开启的", "opened by you")
    }

    /// A colleague's line: who asked for it, and the task they wrote.
    pub fn asked_by(&self, asker: &str) -> String {
        match self.lang {
            Lang::Zh => format!("受 {asker} 之托"),
            Lang::En => format!("for {asker}"),
        }
    }

    pub fn this_machine(&self) -> &'static str {
        self.pick("本机", "this machine")
    }

    // How things are.

    pub fn here_busy(&self, here: usize, busy: usize) -> String {
        match (self.lang, busy) {
            (Lang::Zh, 0) => format!("{here} 位在场"),
            (Lang::Zh, _) => format!("{here} 位在场 · {busy} 位在忙"),
            (Lang::En, 0) => format!("{here} here"),
            (Lang::En, _) => format!("{here} here · {busy} busy"),
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
            Lang::Zh => format!("{entries} 条 · {writers} 位写过"),
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
        self.pick("在忙", "busy")
    }

    pub fn waiting_for_user(&self) -> &'static str {
        self.pick("等你回答", "waiting for you")
    }

    pub fn idle(&self) -> &'static str {
        self.pick("空闲", "idle")
    }

    pub fn asleep(&self) -> &'static str {
        self.pick("睡着", "asleep")
    }

    pub fn left(&self) -> &'static str {
        self.pick("已离开", "left")
    }

    pub fn moved_to(&self, town: Option<&str>) -> String {
        match (self.lang, town) {
            (Lang::Zh, Some(town)) => format!("去了 {town}"),
            (Lang::Zh, None) => "离开了项目".to_string(),
            (Lang::En, Some(town)) => format!("moved to {town}"),
            (Lang::En, None) => "moved out".to_string(),
        }
    }

    pub fn using_desk(&self) -> &'static str {
        self.pick("正在用这台机器的桌面", "using this machine's desktop")
    }

    pub fn no_actions(&self) -> &'static str {
        self.pick("还没有动作", "no actions yet")
    }

    /// A job running: where, for how long, and its first line.
    pub fn running_for(&self, machine: &str, for_how_long: &str) -> String {
        match self.lang {
            Lang::Zh => format!("在 {machine} 上跑了 {for_how_long}"),
            Lang::En => format!("running on {machine} for {for_how_long}"),
        }
    }

    pub fn started_by(&self, who: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{who} 起的"),
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
        self.pick("没有回答", "did not answer")
    }

    pub fn alone(&self) -> &'static str {
        self.pick("独自一台", "on its own")
    }

    pub fn connecting(&self) -> &'static str {
        self.pick("正在连中枢", "connecting to the hub")
    }

    pub fn hub_online(&self) -> &'static str {
        self.pick("中枢在线", "hub online")
    }

    pub fn hub_stopped(&self) -> &'static str {
        self.pick("中枢拒绝了这台机器", "the hub refused this machine")
    }

    pub fn no_record_here(&self, machine: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{machine} 上没有这个项目的记录，所以问了每台机器"),
            Lang::En => format!("{machine} keeps no record of this project, so every machine was asked"),
        }
    }

    pub fn opened(&self, by: &str, at: &str, version: u32) -> String {
        match self.lang {
            Lang::Zh => format!("{by}于 {at} 开启 · 第 {version} 版"),
            Lang::En => format!("opened by {by} {at} · version {version}"),
        }
    }

    pub fn last_entry(&self, by: &str, at: &str) -> String {
        match self.lang {
            Lang::Zh => format!("最近一条：{by}，{at}"),
            Lang::En => format!("latest by {by}, {at}"),
        }
    }

    pub fn decisions_unread(&self, machine: &str, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("在 {machine} 上，现在读不到：{why}"),
            Lang::En => format!("on {machine}, not readable now: {why}"),
        }
    }

    pub fn no_decisions(&self) -> &'static str {
        self.pick("立项属性没写定论在哪", "the project does not say where its decisions are")
    }

    pub fn nothing_yet(&self) -> &'static str {
        self.pick("还没有", "none yet")
    }

    pub fn no_projects(&self) -> &'static str {
        self.pick("这台机器上还没有项目。按 p 开一个。", "No projects on this machine yet. Press p to open one.")
    }

    pub fn nobody_outside(&self) -> &'static str {
        self.pick("没有", "nobody")
    }

    pub fn as_of(&self, at: &str) -> String {
        match self.lang {
            Lang::Zh => format!("以下是 {at} 时的样子"),
            Lang::En => format!("as it was at {at}"),
        }
    }

    pub fn asking(&self) -> &'static str {
        self.pick("正在问各台机器…", "asking the machines…")
    }

    pub fn node_unreachable(&self) -> &'static str {
        self.pick("连不上本机的节点", "cannot reach this machine's node")
    }

    // Keys.

    pub fn key_open(&self) -> &'static str {
        self.pick("打开", "open")
    }

    pub fn key_talk(&self) -> &'static str {
        self.pick("对话", "talk")
    }

    pub fn key_new_agent(&self) -> &'static str {
        self.pick("请一位智能体", "new agent")
    }

    pub fn key_new_project(&self) -> &'static str {
        self.pick("开项目", "new project")
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
        self.pick("看睡着的", "show asleep")
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
            Lang::Zh => format!("已在 {how_long}"),
            Lang::En => format!("here {how_long}"),
        }
    }

    pub fn nobody_here(&self) -> String {
        self.pick("没有人在", "nobody here").to_string()
    }

    pub fn no_agents_here(&self) -> &'static str {
        self.pick(
            "现在没有智能体在这个项目里。按 n 请一位进来。",
            "No agents in this project now. Press n to bring one in.",
        )
    }

    pub fn no_ledger_here(&self, machine: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{machine} 上没有它的账本"),
            Lang::En => format!("{machine} keeps no ledger of it"),
        }
    }

    pub fn heard(&self) -> &'static str {
        self.pick("送进来的话", "Words it heard")
    }

    pub fn online_since(&self, since: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{since} 起在线"),
            Lang::En => format!("online since {since}"),
        }
    }

    pub fn not_asked(&self, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("没问到：{why}"),
            Lang::En => format!("not asked: {why}"),
        }
    }

    pub fn own_sessions(&self) -> &'static str {
        self.pick("你自己开的会话", "Your own sessions")
    }

    /// This computer's agents, a line for each of their logins.
    pub fn agents_here(&self) -> &'static str {
        self.pick("这台电脑上的智能体", "Agents on this computer")
    }

    pub fn agent_not_installed(&self) -> &'static str {
        self.pick("没装", "not installed")
    }

    pub fn agent_logged_in(&self) -> &'static str {
        self.pick("已登录", "logged in")
    }

    pub fn agent_logged_out(&self) -> &'static str {
        self.pick("没登录", "not logged in")
    }

    pub fn agent_login_unknown(&self) -> &'static str {
        self.pick("登没登录说不准", "login not known")
    }

    /// How the World knows of a login.
    pub fn login_found(&self, found: fastctx_types::agents::HomeFound) -> &'static str {
        use fastctx_types::agents::HomeFound;
        match found {
            HomeFound::Default => self.pick("默认", "default"),
            HomeFound::Found => self.pick("自动找到", "found"),
            HomeFound::Added => self.pick("你添加的", "added by you"),
        }
    }

    /// Whether sessions the user opens in a login have the World's tools.
    pub fn login_tools(&self, standing: fastctx_types::agents::Integration) -> &'static str {
        use fastctx_types::agents::Integration;
        match standing {
            Integration::In => self.pick("已接入", "tools in"),
            Integration::Out => self.pick("没接入", "tools not in"),
            Integration::Old => self.pick("里是旧版 fastctx", "an old fastctx is in"),
            Integration::Absent => self.pick("没装", "not installed"),
        }
    }

    pub fn login_needs_consent(&self) -> &'static str {
        self.pick("还没许可不经审批干活", "not yet allowed to work without asking")
    }

    /// When the usage shown was read: `3 分钟前读到`.
    pub fn read_ago(&self, ago: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{ago}读到"),
            Lang::En => format!("read {ago}"),
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
            Lang::Zh => format!("额度没读到：{why}"),
            Lang::En => format!("limits not read: {why}"),
        }
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
            (Lang::Zh, Standing::Out) => format!("{host} 没接入"),
            (Lang::Zh, Standing::Absent) => format!("{host} 没装"),
            (Lang::Zh, Standing::Old) => format!("{host} 里是旧版 fastctx"),
            (Lang::En, Standing::In) => format!("{host}: in"),
            (Lang::En, Standing::Out) => format!("{host}: not in"),
            (Lang::En, Standing::Absent) => format!("{host}: not installed"),
            (Lang::En, Standing::Old) => format!("{host}: an old fastctx is in"),
        }
    }

    pub fn tool_standing(&self, host: &str, standing: Standing, verified: Option<bool>) -> String {
        if standing != Standing::In {
            return self.standing(host, standing);
        }
        match (self.lang, host, verified) {
            (Lang::Zh, "Codex", Some(true)) => "Codex 已接管工具".into(),
            (Lang::Zh, _, Some(true)) => format!("{host} 已设工具拒绝"),
            (Lang::Zh, _, _) => format!("{host} 工具待核对"),
            (Lang::En, "Codex", Some(true)) => "Codex: tools replaced".into(),
            (Lang::En, _, Some(true)) => format!("{host}: deny rules set"),
            (Lang::En, _, _) => format!("{host}: tools need checking"),
        }
    }

    pub fn tools_unverified(&self) -> &'static str {
        self.pick("配置已提交；工具接管检查未通过。", "Configuration committed; native tool verification did not pass.")
    }

    /// On the World page, while an old fastctx's tools are still in Codex.
    pub fn old_tools(&self) -> &'static str {
        self.pick(
            "这台机器的 Codex 里还是旧版 fastctx 的工具：按 m 到机器页，按 u 换成新版",
            "This machine's Codex still has an old fastctx's tools: press m, then u, to put the new ones in",
        )
    }

    pub fn applying(&self) -> &'static str {
        self.pick("正在接入…", "Putting the World's tools in…")
    }

    pub fn unapplying(&self) -> &'static str {
        self.pick("正在拿出来…", "Taking the World's tools out…")
    }

    pub fn applied(&self, files: usize, commands: usize) -> String {
        match (self.lang, files + commands) {
            (Lang::Zh, 0) => "早就接好了，什么都没改。".to_string(),
            (Lang::En, 0) => "Already in place; nothing changed.".to_string(),
            (Lang::Zh, _) if commands == 0 => {
                format!("接好了：改了 {files} 个文件。之后新开的会话就有世界的工具，已经开着的不变。")
            }
            (Lang::Zh, _) => format!(
                "接好了：改了 {files} 个文件，跑了 {commands} 条 claude 命令。之后新开的会话就有世界的工具，已经开着的不变。"
            ),
            (Lang::En, _) => format!(
                "In: {files} files changed, {commands} claude commands run. Sessions opened from now on have the World's tools."
            ),
        }
    }

    pub fn unapplied(&self, files: usize, commands: usize) -> String {
        match (self.lang, files + commands) {
            (Lang::Zh, 0) => "里面本来就没有世界的东西。".to_string(),
            (Lang::En, 0) => "Nothing of the World's was in.".to_string(),
            (Lang::Zh, _) => format!("拿出来了：改了 {files} 个文件。之后新开的会话不再有世界的工具。"),
            (Lang::En, _) => {
                format!("Out: {files} files changed. Sessions opened from now on no longer have the World's tools.")
            }
        }
    }

    pub fn needs_raise(&self, current: i64, requested: u64) -> String {
        match self.lang {
            Lang::Zh => format!(
                "Codex 的输出上限设成了 {current}，世界的一次回答要 {requested}，所以什么都没改。这个上限管着 Codex 的所有工具，按 U 同意调高并接入。"
            ),
            Lang::En => format!(
                "Codex's output limit is {current} and the World's answers need {requested}, so nothing changed. It applies to every tool in Codex; press U to raise it and go on."
            ),
        }
    }

    /// What was done with what an old fastctx left on this machine.
    pub fn legacy_cleaned(&self, tidied: usize, archived: usize, place: &str, waiting: usize) -> String {
        match self.lang {
            Lang::Zh => {
                let mut said = format!(
                    "旧版 fastctx 留下的：{tidied} 处原样未动的已收拾，{archived} 样归档在 {place}，删不删由你"
                );
                if waiting > 0 {
                    said.push_str(&format!("；还有 {waiting} 样在用，等不用了再收"));
                }
                said
            }
            Lang::En => {
                let mut said = format!(
                    "Left by an old fastctx: {tidied} untouched files tidied, {archived} archived in {place}, yours to delete"
                );
                if waiting > 0 {
                    said.push_str(&format!("; {waiting} still in use, put away once they are not"));
                }
                said
            }
        }
    }

    pub fn left_alone(&self, location: &str, reason: &str) -> String {
        match self.lang {
            Lang::Zh => format!("留着没动：{location}（{reason}）"),
            Lang::En => format!("left alone: {location} ({reason})"),
        }
    }

    pub fn key_apply(&self) -> &'static str {
        self.pick("接入", "put in")
    }

    pub fn key_replace_old(&self) -> &'static str {
        self.pick("换成新版", "replace the old fastctx")
    }

    pub fn key_unapply(&self) -> &'static str {
        self.pick("拿出来", "take out")
    }

    /// The keys on one login: put the World's tools into it, take them out.
    pub fn key_apply_login(&self) -> &'static str {
        self.pick("接入这个登录", "put in this login")
    }

    pub fn key_unapply_login(&self) -> &'static str {
        self.pick("从这个登录拿出来", "take out of this login")
    }

    pub fn key_raise(&self) -> &'static str {
        self.pick("调高并接入", "raise and put in")
    }

    pub fn this_machine_link(&self) -> &'static str {
        self.pick("这台机器", "This machine")
    }

    pub fn link_alone(&self) -> String {
        self.pick(
            "还没登录任何中枢：它自己一台。按 l 用中枢地址和账号口令登录，或者按 k 用另一台设备给的入网令牌登录。",
            "Logged in to no hub: it is on its own. Log in with l, with the hub's address and an account's password, or with k and a token another device made.",
        )
        .to_string()
    }

    pub fn link_connecting(&self, since: &str, error: Option<&str>) -> String {
        match (self.lang, error) {
            (Lang::Zh, Some(error)) => format!("{since} 起在连中枢，还没连上：{error}"),
            (Lang::Zh, None) => format!("{since} 起在连中枢"),
            (Lang::En, Some(error)) => format!("connecting to the hub since {since}, not yet: {error}"),
            (Lang::En, None) => format!("connecting to the hub since {since}"),
        }
    }

    pub fn link_online(
        &self,
        hub: &str,
        relay: Option<&str>,
        route: &Route,
        since: &str,
    ) -> String {
        if let Some(relay) = relay {
            return match (self.lang, route) {
                (Lang::Zh, Route::Local) => format!("{since} 起连着本机上的转接站 {relay}"),
                (Lang::Zh, Route::Card { name }) => {
                    format!("{since} 起直接经网卡「{name}」连着转接站 {relay}（{hub}）")
                }
                (Lang::Zh, Route::Proxy { proxy }) => format!("{since} 起经代理 {proxy} 连着转接站 {relay}（{hub}）"),
                (Lang::Zh, Route::System) => format!("{since} 起按系统的路由连着转接站 {relay}（{hub}）"),
                (Lang::En, Route::Local) => format!("connected since {since} to the relay on this machine, {relay}"),
                (Lang::En, route) => format!("connected since {since} through the relay {relay} at {hub}, {route}"),
            };
        }
        match (self.lang, route) {
            (Lang::Zh, Route::Local) => format!("{since} 起连着本机上的中枢 {hub}"),
            (Lang::Zh, Route::Card { name }) => format!("{since} 起直接经网卡「{name}」连着中枢 {hub}"),
            (Lang::Zh, Route::Proxy { proxy }) => format!("{since} 起经代理 {proxy} 连着中枢 {hub}"),
            (Lang::Zh, Route::System) => format!("{since} 起按系统的路由连着中枢 {hub}"),
            (Lang::En, Route::Local) => format!("connected since {since} to the hub on this machine, {hub}"),
            (Lang::En, route) => format!("connected since {since} to the hub at {hub}, {route}"),
        }
    }

    pub fn link_stopped(&self, reason: &str) -> String {
        match self.lang {
            Lang::Zh => format!("中枢不再接受这台机器：{reason}"),
            Lang::En => format!("the hub no longer takes this machine: {reason}"),
        }
    }

    // The conversation.

    pub fn say_to(&self, agent: &str) -> String {
        match self.lang {
            Lang::Zh => format!("对 {agent} 说…"),
            Lang::En => format!("Say something to {agent}…"),
        }
    }

    pub fn sending(&self) -> &'static str {
        self.pick("送出中…", "sending…")
    }

    pub fn not_sent(&self, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("没送出去：{why}"),
            Lang::En => format!("not sent: {why}"),
        }
    }

    pub fn joined_turn(&self) -> &'static str {
        self.pick("并进了它正在跑的这一轮", "went into the turn it is running")
    }

    pub fn handed(&self) -> &'static str {
        self.pick("已交给它的会话", "handed to its session")
    }

    pub fn landing_unknown(&self, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("话交出去了，但不知道它进没进去：{why}"),
            Lang::En => format!("handed over, but whether it went in is not known: {why}"),
        }
    }

    pub fn sent_to(&self, machine: &str) -> String {
        match self.lang {
            Lang::Zh => format!("已寄往 {machine}，等那台机器收到再送进去"),
            Lang::En => format!("sent to {machine}; it goes in once that machine has it"),
        }
    }

    pub fn waiting_because(&self, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("在等它来取：{why}"),
            Lang::En => format!("waiting for it: {why}"),
        }
    }

    pub fn waiting_for_call(&self) -> &'static str {
        self.pick("在等它下一次用世界的工具时带走", "waiting for it to take them with its next call of a World tool")
    }

    pub fn no_turn_end_known(&self) -> &'static str {
        self.pick("；它的宿主不告诉世界一轮何时结束", "; its host does not tell the World when its turns end")
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
        self.pick("没让它用", "refused")
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
            Lang::Zh => format!("转到后台 {job}"),
            Lang::En => format!("in the background as {job}"),
        }
    }

    pub fn in_directory(&self, cwd: &str) -> String {
        match self.lang {
            Lang::Zh => format!("在 {cwd}"),
            Lang::En => format!("in {cwd}"),
        }
    }

    pub fn more_lines_before(&self, lines: usize) -> String {
        match self.lang {
            Lang::Zh => format!("…前面还有 {lines} 行"),
            Lang::En => format!("… {lines} more lines before"),
        }
    }

    pub fn more_lines_after(&self, lines: usize) -> String {
        match self.lang {
            Lang::Zh => format!("…还有 {lines} 行"),
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

    pub fn thinking(&self) -> &'static str {
        self.pick("思考", "Thinking")
    }

    pub fn thinking_withheld(&self) -> &'static str {
        self.pick("没有交出内容", "its words were not given")
    }

    pub fn plan(&self) -> &'static str {
        self.pick("它提出的计划", "Its proposed plan")
    }

    pub fn steps(&self) -> &'static str {
        self.pick("步骤", "Steps")
    }

    pub fn compacted(&self) -> &'static str {
        self.pick("—— 上下文压缩过了 ——", "—— context compacted ——")
    }

    pub fn turn_done(&self) -> &'static str {
        self.pick("这一轮做完了", "this turn is done")
    }

    pub fn turn_stopped(&self) -> &'static str {
        self.pick("这一轮被停下了", "this turn was stopped")
    }

    pub fn tool_cut(&self) -> &'static str {
        self.pick("这一轮结束时还没跑完", "the turn ended before it did")
    }

    pub fn turn_failed(&self, reason: Option<&str>) -> String {
        match (self.lang, reason) {
            (Lang::Zh, Some(reason)) => format!("这一轮失败：{reason}"),
            (Lang::Zh, None) => "这一轮失败".to_string(),
            (Lang::En, Some(reason)) => format!("turn failed: {reason}"),
            (Lang::En, None) => "turn failed".to_string(),
        }
    }

    pub fn turn_interrupted(&self, reason: Option<&str>) -> String {
        match (self.lang, reason) {
            (Lang::Zh, Some(reason)) => format!("这一轮被打断：{reason}"),
            (Lang::Zh, None) => "这一轮被打断".to_string(),
            (Lang::En, Some(reason)) => format!("turn interrupted: {reason}"),
            (Lang::En, None) => "turn interrupted".to_string(),
        }
    }

    pub fn turn_ended_as(&self, how: &str) -> String {
        match self.lang {
            Lang::Zh => format!("这一轮结束：{how}"),
            Lang::En => format!("turn ended: {how}"),
        }
    }

    /// What kind of fact about a session a health line reports; the host's
    /// own words follow it.
    pub fn health(&self, kind: HealthKind) -> &'static str {
        match kind {
            HealthKind::Reconnecting => self.pick("在重连模型", "reconnecting to the model"),
            HealthKind::Safety => {
                self.pick("模型服务商扣下了输出做安全检查", "the provider is holding output for a safety check")
            }
            HealthKind::Rerouted => {
                self.pick("模型服务商把这一轮换给了别的模型", "the provider moved the turn to another model")
            }
            HealthKind::ToolsMissing => {
                self.pick("这段会话里没有世界的工具", "the World's tools are not in this session")
            }
            HealthKind::ToolServer => {
                self.pick("会话的另一个工具服务没起来", "another tool server of the session did not start")
            }
            HealthKind::Warning => self.pick("宿主提醒", "the host warns"),
            HealthKind::Error => self.pick("宿主报错", "the host reports an error"),
        }
    }

    pub fn stopping(&self) -> &'static str {
        self.pick("已请它停下这一轮", "asked it to stop its turn")
    }

    pub fn no_turn_running(&self) -> &'static str {
        self.pick("它现在没有在跑的一轮", "it is running no turn now")
    }

    pub fn not_stopped(&self, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("没能停下：{why}"),
            Lang::En => format!("could not stop it: {why}"),
        }
    }

    pub fn opening_session(&self) -> &'static str {
        self.pick("正在接上这段会话…", "opening the session…")
    }

    pub fn nothing_said_yet(&self) -> &'static str {
        self.pick("这段会话里还没有能看到的内容。", "Nothing in this session can be shown yet.")
    }

    pub fn more_below(&self) -> &'static str {
        self.pick("下面还有 ↓", "more below ↓")
    }

    /// Rows below a conversation that did not fit.
    pub fn more_rows(&self, count: usize) -> String {
        match self.lang {
            Lang::Zh => format!("还有 {count} 项"),
            Lang::En => format!("{count} more"),
        }
    }

    pub fn pasting(&self, count: usize) -> String {
        match self.lang {
            Lang::Zh => format!("正在接收粘贴的内容（{count} 字）…"),
            Lang::En => format!("receiving a paste ({count} characters)…"),
        }
    }

    pub fn watch_broke(&self, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("看着它的连接断了：{why}"),
            Lang::En => format!("the watch broke: {why}"),
        }
    }

    pub fn watch_ended(&self) -> &'static str {
        self.pick("看着它的连接结束了", "the watch ended")
    }

    // Journals.

    pub fn described(&self, by: &str, at: &str) -> String {
        match self.lang {
            Lang::Zh => format!("这段描述由 {by} 于 {at} 写下"),
            Lang::En => format!("described by {by} {at}"),
        }
    }

    pub fn earlier_entries(&self, count: usize) -> String {
        match self.lang {
            Lang::Zh => format!("更早还有 {count} 条，按 [ 往前翻"),
            Lang::En => format!("{count} earlier entries; press [ to read back"),
        }
    }

    pub fn entries_shown(&self, first: usize, last: usize, total: usize) -> String {
        match self.lang {
            Lang::Zh => format!("第 {first}–{last} 条，共 {total} 条"),
            Lang::En => format!("entries {first}–{last} of {total}"),
        }
    }

    pub fn reading(&self) -> &'static str {
        self.pick("正在读…", "reading…")
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
            Lang::Zh => format!("后来提到它的：{ids}"),
            Lang::En => format!("mentioned later by {ids}"),
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
        self.pick("不进项目", "no project")
    }

    pub fn form_open_agent(&self) -> &'static str {
        self.pick("请一位智能体", "New agent")
    }

    pub fn form_new_project(&self) -> &'static str {
        self.pick("开一个项目", "New project")
    }

    pub fn form_edit_project(&self, town: &str) -> String {
        match self.lang {
            Lang::Zh => format!("改 {town} 的立项属性"),
            Lang::En => format!("Edit the record of {town}"),
        }
    }

    pub fn form_login(&self) -> &'static str {
        self.pick("登录一个账号", "Log in to an account")
    }

    pub fn form_token_login(&self) -> &'static str {
        self.pick("用入网令牌登录", "Log in with a token")
    }

    pub fn form_token(&self) -> &'static str {
        self.pick("给另一台设备的入网令牌", "A token for another device")
    }

    pub fn form_logout(&self) -> &'static str {
        self.pick("退出一个账号", "Log out of an account")
    }

    pub fn form_remove(&self, name: &str, account: &str) -> String {
        match self.lang {
            Lang::Zh => format!("把 {name} 移出账号 {account}"),
            Lang::En => format!("Remove {name} from the account {account}"),
        }
    }

    pub fn remove_warning(&self, name: &str, account: &str) -> String {
        match self.lang {
            Lang::Zh => format!(
                "移除之后，{name} 不再替账号 {account} 干活：是设备，就退出了这个账号；是这个账号的机器，就离开了中枢，要回来得重新登录。这用在设备丢了，或者进来的不是你的时候。"
            ),
            Lang::En => format!(
                "Once removed, {name} no longer works for the account {account}: a device is logged out of it; a machine of the account leaves the hub, and comes back only by logging in again. This is for a lost device, or one that came in and was not you."
            ),
        }
    }

    pub fn logout_warning(&self) -> &'static str {
        self.pick(
            "退出之后，这个账号能用的机器、智能体与项目从这台电脑上都看不到了；这台电脑上替它干活的会话也不再替它干活。这台电脑登记在这个账号名下时，它也就离开了那个中枢。",
            "Once logged out, the machines, agents and projects of the account are out of sight from this computer, and its sessions here no longer work for it. Where this computer is registered under the account, it leaves that hub too.",
        )
    }

    pub fn register_no(&self) -> &'static str {
        self.pick("只登录", "log in only")
    }

    pub fn register_yes(&self) -> &'static str {
        self.pick("登录并登记成这个账号的机器", "log in and register as the account's machine")
    }

    pub fn token_for_phone(&self) -> &'static str {
        self.pick("手机：只登录", "a phone: log in only")
    }

    pub fn token_for_computer(&self) -> &'static str {
        self.pick("另一台电脑：只登录", "another computer: log in only")
    }

    pub fn token_for_server(&self) -> &'static str {
        self.pick("没有界面的服务器：装上并登记成机器", "a server with no screen: install and register it")
    }

    pub fn hours(&self, hours: &str) -> String {
        match self.lang {
            Lang::Zh => format!("{hours} 小时"),
            Lang::En if hours == "1" => "1 hour".to_string(),
            Lang::En => format!("{hours} hours"),
        }
    }

    pub fn token_how(&self, server: bool) -> &'static str {
        if server {
            self.pick(
                "在服务器上以要装它的用户粘贴运行这一整行：它核对发布包的摘要，装上 fastctx，登记成这个账号的机器，再装开机服务。令牌只能用一次。",
                "Paste and run this whole line on the server, as the user it is for: it checks the release's digest, installs fastctx, registers it as a machine of the account and installs the boot service. The token works once.",
            )
        } else {
            self.pick(
                "在另一台设备上用它登录：TUI 里按 m 再按 k，或者运行 fastctx login --token <这串令牌>。它只能用一次。",
                "Log in with it on the other device: press m then k in its TUI, or run fastctx login --token <this token>. It works once.",
            )
        }
    }

    pub fn token_until(&self, until: &str) -> String {
        match self.lang {
            Lang::Zh => format!("有效到 {until}"),
            Lang::En => format!("valid until {until}"),
        }
    }

    pub fn logged_in(&self, hub: &str, name: &str, account: &str) -> String {
        match self.lang {
            Lang::Zh => format!("已登录中枢 {hub} 上的账号 {account}，这台电脑在那里叫 {name}。"),
            Lang::En => format!("Logged in to the account {account} on the hub {hub}; this computer is {name} there."),
        }
    }

    pub fn login_needs(&self) -> &'static str {
        self.pick("要写中枢地址、账号和口令", "the hub, the account and the password are needed")
    }

    pub fn token_needed(&self) -> &'static str {
        self.pick("要粘贴入网令牌", "paste the token")
    }

    pub fn hub_section(&self, hub: &str) -> String {
        match self.lang {
            Lang::Zh => format!("中枢 {hub}"),
            Lang::En => format!("The hub {hub}"),
        }
    }

    pub fn logins_line(&self, logins: &str, home: Option<&str>) -> String {
        match (self.lang, home) {
            (Lang::Zh, Some(home)) => format!("登录着 {logins}；这台电脑登记在 {home} 名下"),
            (Lang::Zh, None) => format!("登录着 {logins}"),
            (Lang::En, Some(home)) => format!("logged in to {logins}; this computer is registered under {home}"),
            (Lang::En, None) => format!("logged in to {logins}"),
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

    /// A choice of an agent lent to this computer's accounts: the machine it
    /// lives on, whose that is, and which agent.
    pub fn lent_choice(&self, machine: &str, owner: Option<&str>, agent: &str) -> String {
        let agent = if agent == "claude" { "Claude" } else if agent == "codex" { "Codex" } else { agent };
        match (self.lang, owner) {
            (Lang::Zh, Some(owner)) => format!("{agent} · {machine}（借自 {owner}）"),
            (Lang::Zh, None) => format!("{agent} · {machine}（借来的）"),
            (Lang::En, Some(owner)) => format!("{agent} on {machine} (lent by {owner})"),
            (Lang::En, None) => format!("{agent} on {machine} (lent)"),
        }
    }

    /// Whether `account` may use a machine: work on it and use every agent
    /// on it, which go together (`world-network.md`, 共享机器与借出智能体).
    pub fn rights(&self, account: &str, hands: bool) -> String {
        match (self.lang, hands) {
            (Lang::Zh, true) => format!("{account} 能用"),
            (Lang::Zh, false) => format!("{account} 不能用"),
            (Lang::En, true) => format!("{account} may use it"),
            (Lang::En, false) => format!("{account} may not use it"),
        }
    }

    pub fn arrivals(&self) -> &'static str {
        self.pick("新进来的", "Who came in")
    }

    pub fn arrival(&self, name: &str, device: bool, account: &str, hub: &str, at: &str) -> String {
        match (self.lang, device) {
            (Lang::Zh, true) => format!("设备 {name} 在 {at} 登录了 {hub} 上的账号 {account}"),
            (Lang::Zh, false) => format!("机器 {name} 在 {at} 登记到了 {hub} 上的账号 {account} 名下"),
            (Lang::En, true) => format!("the device {name} logged in to {account} on {hub} at {at}"),
            (Lang::En, false) => format!("the machine {name} was registered under {account} on {hub} at {at}"),
        }
    }

    pub fn key_login(&self) -> &'static str {
        self.pick("登录", "log in")
    }

    pub fn key_token_login(&self) -> &'static str {
        self.pick("用令牌登录", "log in with a token")
    }

    pub fn key_token(&self) -> &'static str {
        self.pick("给设备的令牌", "token")
    }

    pub fn key_logout(&self) -> &'static str {
        self.pick("退出账号", "log out")
    }

    pub fn key_remove(&self) -> &'static str {
        self.pick("移出账号", "remove")
    }

    pub fn not_me_reason(&self) -> &'static str {
        self.pick("用户说进来的不是他", "the user said it was not them")
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
            (Lang::Zh, Kept::Already | Kept::Installed) => "开机就连着：系统服务会一直让它在线，登不登录都一样。".to_string(),
            (Lang::En, Kept::Already | Kept::Installed) => {
                "Kept connected: a system service keeps it online from boot, signed in or not.".to_string()
            }
            (Lang::Zh, Kept::Declined) => {
                "没装上开机服务：它只在你登录时连着。以管理员身份跑 fastctx service node install 可以补装。".to_string()
            }
            (Lang::En, Kept::Declined) => "No boot service: it is connected only while you are signed in. Run fastctx service node install as administrator to add it.".to_string(),
            (Lang::Zh, Kept::Unsupported) => "这个系统没有给节点用的服务：它只在你登录时连着。".to_string(),
            (Lang::En, Kept::Unsupported) => "This system has no service for the node: it is connected only while you are signed in.".to_string(),
            (Lang::Zh, Kept::Command(command)) => format!("要它开机就连着，还需装系统服务：{command}"),
            (Lang::En, Kept::Command(command)) => format!("To keep it connected from boot, install the service: {command}"),
        }
    }

    pub fn not_kept(&self, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("开机服务没装上：{why}"),
            Lang::En => format!("The boot service was not installed: {why}"),
        }
    }

    pub fn working(&self) -> &'static str {
        self.pick("正在办…", "working…")
    }

    pub fn say_something_first(&self) -> &'static str {
        self.pick("先写下要对它说的第一句话", "write the first words to say to it")
    }

    pub fn hours_wrong(&self) -> &'static str {
        self.pick("有效时长要写成小时数，比如 24", "write how long it stays valid in hours, such as 24")
    }

    pub fn name_and_what_needed(&self) -> &'static str {
        self.pick("名字和「它是什么」都要写", "a name and what it is are both needed")
    }

    pub fn place_wrong(&self, line: &str) -> String {
        match self.lang {
            Lang::Zh => format!("这一行要写成「是什么 | 机器 | 路径」：{line}"),
            Lang::En => format!("write this line as \"what | machine | path\": {line}"),
        }
    }

    pub fn canon_wrong(&self, canon: &str) -> String {
        match self.lang {
            Lang::Zh => format!("定论在哪要写成「机器 | 路径」：{canon}"),
            Lang::En => format!("write where the decisions are as \"machine | path\": {canon}"),
        }
    }

    pub fn field(&self, field: FieldName) -> &'static str {
        match field {
            FieldName::Agent => self.pick("哪一家", "Agent"),
            FieldName::Machine => self.pick("在哪台机器", "Machine"),
            FieldName::Project => self.pick("进哪个项目", "Project"),
            FieldName::FirstWords => self.pick("第一句话", "First words"),
            FieldName::Slug => self.pick("简称", "Short name"),
            FieldName::Name => self.pick("名字", "Name"),
            FieldName::What => self.pick("它是什么", "What it is"),
            FieldName::Machines => self.pick("机器", "Machines"),
            FieldName::Places => self.pick("在哪里能找到什么", "Where things are"),
            FieldName::Canon => self.pick("定论在哪", "Decisions"),
            FieldName::HubAddress => self.pick("中枢地址", "Hub"),
            FieldName::Account => self.pick("账号", "Account"),
            FieldName::Password => self.pick("口令", "Password"),
            FieldName::Register => self.pick("这台电脑", "This computer"),
            FieldName::LoginName => self.pick("这台电脑叫什么", "This computer's name"),
            FieldName::Token => self.pick("入网令牌", "Token"),
            FieldName::HubAccount => self.pick("哪个账号", "Account"),
            FieldName::TokenFor => self.pick("给谁用", "For"),
            FieldName::Hours => self.pick("有效多久", "Valid for"),
            FieldName::Reason => self.pick("为什么", "Why"),
        }
    }

    pub fn hint(&self, field: FieldName) -> Option<&'static str> {
        Some(match field {
            FieldName::Machine => self.pick("只列在线的机器", "only machines online"),
            FieldName::Project => self.pick("只列横跨那台机器的项目", "only projects that span that machine"),
            FieldName::FirstWords => {
                self.pick("Ctrl+J 换行，或在行尾打 \\ 再按 Enter", "Ctrl+J for a new line, or \\ then Enter")
            }
            FieldName::Slug => {
                self.pick("小写字母、数字与连字符，比如 mark50", "lowercase letters, digits and hyphens, like mark50")
            }
            FieldName::What => self.pick("一两句话：这个项目为了什么", "a sentence or two: what the project is for"),
            FieldName::Machines => self.pick("← → 移动，Space 勾选", "← → to move, Space to tick"),
            FieldName::Places => {
                self.pick("一行一样东西：是什么 | 机器 | 路径", "one thing a line: what | machine | path")
            }
            FieldName::Canon => self.pick("机器 | 路径；没有就空着", "machine | path; empty when there are none"),
            FieldName::HubAddress => {
                self.pick("主机:端口，比如 hub.example.com:7443", "host:port, such as hub.example.com:7443")
            }
            FieldName::Register => self.pick("← → 选择", "← → to choose"),
            FieldName::LoginName => self.pick(
                "可以空着，用这台电脑的主机名；它在每个中枢上都叫这个名字",
                "may be empty: the host name is used; it has this name on every hub",
            ),
            FieldName::Token => self.pick(
                "在一台已经登录的设备上生成，只能用一次",
                "made on a device already logged in; it works once",
            ),
            _ => return None,
        })
    }

    // More keys.

    pub fn key_present_only(&self) -> &'static str {
        self.pick("只看在场的", "present only")
    }

    pub fn key_read(&self) -> &'static str {
        self.pick("阅读", "read")
    }

    pub fn key_new_agent_here(&self) -> &'static str {
        self.pick("请一位智能体", "new agent")
    }

    pub fn key_edit_project(&self) -> &'static str {
        self.pick("改立项属性", "edit record")
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
        self.pick("停下这一轮", "stop the turn")
    }

    pub fn key_watch_again(&self) -> &'static str {
        self.pick("重新接上", "watch again")
    }

    pub fn key_attach(&self) -> &'static str {
        self.pick("在它自己的界面里打开", "open in its own interface")
    }

    pub fn attach_elsewhere(&self, machine: &str) -> String {
        match self.lang {
            Lang::Zh => format!("它住在 {machine}，要在那台机器上打开"),
            Lang::En => format!("it lives on {machine}; open it there"),
        }
    }

    pub fn attach_failed(&self, why: &str) -> String {
        match self.lang {
            Lang::Zh => format!("没能打开：{why}"),
            Lang::En => format!("could not open it: {why}"),
        }
    }

    pub fn attach_exited(&self, code: Option<i32>) -> String {
        match (self.lang, code) {
            (Lang::Zh, Some(code)) => format!("它自己的界面以 {code} 退出"),
            (Lang::Zh, None) => "它自己的界面被中止了".to_string(),
            (Lang::En, Some(code)) => format!("its own interface exited with {code}"),
            (Lang::En, None) => "its own interface was ended".to_string(),
        }
    }

    pub fn key_colleagues(&self) -> &'static str {
        self.pick("它请的同事", "its colleagues")
    }

    pub fn key_open_colleague(&self) -> &'static str {
        self.pick("和这位同事说话", "talk to this colleague")
    }

    pub fn key_back_to_typing(&self) -> &'static str {
        self.pick("回到输入", "back to typing")
    }

    pub fn key_earlier(&self) -> &'static str {
        self.pick("更早的", "earlier")
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
            Finish::Open => self.pick("请它进来", "bring it in"),
            Finish::Create => self.pick("开项目", "open it"),
            Finish::Save => self.pick("写下新的一版", "write the new version"),
            Finish::Login => self.pick("登录", "log in"),
            Finish::Token => self.pick("生成", "make it"),
            Finish::Logout => self.pick("退出登录", "log out"),
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
            Lang::Zh => format!("看不到它请的同事和它起的、还在跑的东西：{why}"),
            Lang::En => format!("cannot see its colleagues or what it started that still runs: {why}"),
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
