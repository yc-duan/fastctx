use fastctx_types::access::HubAccount;
use fastctx_types::agents::LimitWindow;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::usage::*;
use fastctx_types::{Call, Reply};
use jiff::Timestamp;
use serde::Serialize;

#[derive(Serialize)]
pub struct Fixtures {
    pub calls: Vec<Call>,
    pub replies: Vec<Reply>,
    pub remote_calls: Vec<RemoteCall>,
    pub remote_replies: Vec<RemoteReply>,
}

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

fn sum(tokens: u64, usd: Option<u64>) -> UsageSum {
    UsageSum { tokens, usd: usd.map(Usd) }
}

pub fn fixtures() -> Fixtures {
    let ada = HubAccount { hub: "w-5be1fcee3751dc7a".parse().unwrap(), account: "ada".parse().unwrap() };
    // Claude Code: the machine's own sessions and an account it assists,
    // in dollars too, its week the login's own seven-day window.
    let claude = UsageReport {
        people: vec![
            UsagePerson { account: None, week: sum(61_200_000, Some(96_400_000)) },
            UsagePerson { account: Some(ada), week: sum(3_900_000, Some(5_790_000)) },
        ],
        hours: vec![
            UsageHour { at: at("2026-10-12T09:00:00Z"), person: 0, sum: sum(1_840_000, Some(2_910_000)) },
            UsageHour { at: at("2026-10-12T09:00:00Z"), person: 1, sum: sum(220_000, Some(310_000)) },
            UsageHour { at: at("2026-10-12T14:00:00Z"), person: 0, sum: sum(960_000, Some(1_480_000)) },
        ],
        week: UsageWeek {
            start: Some(at("2026-10-08T18:00:00Z")),
            end: Some(at("2026-10-15T18:00:00Z")),
            window: true,
            models: vec![
                ModelUse { model: "claude-opus-5-5".into(), sum: sum(50_800_000, Some(79_500_000)) },
                ModelUse { model: "claude-haiku-5-5".into(), sum: sum(14_300_000, Some(22_690_000)) },
            ],
            tokens: Tokens {
                input: 650_000,
                cache_read: 58_900_000,
                cache_write: 3_900_000,
                output: 1_650_000,
                reasoning: 0,
            },
            kinds_usd: Some(KindsUsd {
                input: Usd(3_060_000),
                cache_read: Usd(30_600_000),
                cache_write: Usd(25_500_000),
                output: Usd(43_030_000),
            }),
        },
        readings: vec![UsageReading {
            at: at("2026-10-12T14:20:00Z"),
            windows: vec![
                LimitWindow {
                    name: "five_hour".into(),
                    minutes: Some(300),
                    used_tenths: 410,
                    resets_at: Some(at("2026-10-12T17:00:00Z")),
                },
                LimitWindow {
                    name: "seven_day".into(),
                    minutes: Some(10_080),
                    used_tenths: 640,
                    resets_at: Some(at("2026-10-15T18:00:00Z")),
                },
            ],
        }],
    };
    // Codex: tokens alone, over the seven days up to now while no window
    // was read.
    let codex = UsageReport {
        people: vec![UsagePerson { account: None, week: sum(17_000_000, None) }],
        hours: vec![UsageHour { at: at("2026-10-12T10:00:00Z"), person: 0, sum: sum(2_400_000, None) }],
        week: UsageWeek {
            start: Some(at("2026-10-05T14:00:00Z")),
            end: Some(at("2026-10-12T14:00:00Z")),
            window: false,
            models: vec![ModelUse { model: "gpt-6.1-sol".into(), sum: sum(17_000_000, None) }],
            tokens: Tokens {
                input: 4_000_000,
                cache_read: 12_000_000,
                cache_write: 0,
                output: 834_000,
                reasoning: 334_000,
            },
            kinds_usd: None,
        },
        readings: vec![],
    };
    let machine: fastctx_types::MachineName = "desktop".parse().unwrap();
    let agent: fastctx_types::AgentName = "claude".parse().unwrap();
    Fixtures {
        calls: vec![Call::AgentUsage {
            machine: machine.clone(),
            agent: agent.clone(),
            dir: "C:\\Users\\me\\.claude".into(),
        }],
        replies: vec![Reply::AgentUsage(Box::new(claude.clone())), Reply::AgentUsage(Box::new(codex))],
        remote_calls: vec![RemoteCall::AgentUsage { agent, dir: "C:\\Users\\me\\.claude".into() }],
        remote_replies: vec![RemoteReply::AgentUsage(Box::new(claude))],
    }
}
