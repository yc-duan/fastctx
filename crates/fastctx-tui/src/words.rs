//! What the screens say, in the user's words (`world-gui.md`, 用词): plain
//! words anyone reads at a glance. Only "World" and "Journal" carry the
//! product's character; residents are named by who they are, such as
//! "Claude · desktop"; a town is a project.
//!
//! Each message keeps its languages side by side, so one is never changed
//! without the other in view.

use std::sync::OnceLock;

use jiff::Timestamp;
use jiff::tz::TimeZone;

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

#[cfg(windows)]
fn system_locale() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
    // LOCALE_NAME_MAX_LENGTH.
    let mut name = [0u16; 85];
    // SAFETY: the buffer is writable and its length is passed with it.
    let length = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
    (length > 1).then(|| String::from_utf16_lossy(&name[..length as usize - 1]))
}

#[cfg(not(windows))]
fn system_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty() && value != "C" && value != "POSIX"))
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
        self.pick("连睡着的一起看", "show sleeping too")
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
