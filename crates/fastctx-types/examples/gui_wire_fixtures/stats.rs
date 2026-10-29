use fastctx_types::stats::*;
use jiff::Timestamp;
use serde::Serialize;

#[derive(Serialize)]
pub struct Fixtures {
    pub stats: Vec<MachineStats>,
}

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

pub fn fixtures() -> Fixtures {
    let windows = MachineStats {
        run: Some("5f3a9c".into()),
        machine: "desktop".parse().unwrap(),
        at: at("2026-10-13T08:30:00Z"),
        boot: Some(at("2026-10-10T01:12:40Z")),
        system: "Windows 11 Pro 24H2 (26200.6584)".into(),
        kernel: None,
        host: "DESKTOP-EXAMPLE".into(),
        arch: "x86_64".into(),
        cpu: Cpu { model: "AMD Ryzen 9 7950X 16-Core Processor".into(), threads: 32, busy: Some(0.18), load: None },
        memory: Memory {
            total: 68_719_476_736,
            used: 30_064_771_072,
            swap_total: 78_383_153_152,
            swap_used: 41_875_931_136,
            room: Some(fastctx_types::stats::MEMORY_ROOM),
            waiting: 2,
        },
        disks: vec![
            Disk {
                at: "C:/".into(),
                label: Some("系统".into()),
                fs: "NTFS".into(),
                total: 1_000_202_039_296,
                free: 212_000_000_000,
            },
            Disk { at: "V:/".into(), label: None, fs: "NTFS".into(), total: 2_000_381_014_016, free: 92_000_000_000 },
        ],
        cards: vec![
            Card {
                name: "Ethernet".into(),
                up: true,
                hardware: true,
                speed: Some(1_000_000_000),
                received: 912_345_678_901,
                sent: 123_456_789_012,
                receiving: Some(1_250_000.0),
                sending: Some(64_000.0),
                addresses: vec!["192.168.1.20".parse().unwrap(), "fd00::20".parse().unwrap()],
            },
            Card {
                name: "Wi-Fi".into(),
                up: false,
                hardware: true,
                speed: None,
                received: 0,
                sent: 0,
                receiving: None,
                sending: None,
                addresses: vec![],
            },
        ],
        gpus: vec![Gpu {
            name: "NVIDIA GeForce RTX 4090".into(),
            memory_total: 25_757_220_864,
            memory_used: 6_442_450_944,
            busy: Some(0.42),
        }],
        lines: vec![Line {
            kept: Some(fastctx_types::stats::LineKept { trips: 41, changes: 3 }),
            hub: "home".parse().unwrap(),
            since: at("2026-10-13T07:58:10Z"),
            up: true,
            through: None,
            trips: vec![
                Trip { at: at("2026-10-13T08:29:40Z"), ms: 38 },
                Trip { at: at("2026-10-13T08:29:50Z"), ms: 41 },
            ],
            changes: vec![
                LineChange::Started { at: at("2026-10-12T22:01:00Z") },
                LineChange::Up { at: at("2026-10-12T22:01:01Z") },
                LineChange::Down { at: at("2026-10-13T07:57:30Z"), why: "nothing came from the hub for 30 s".into() },
                LineChange::Up { at: at("2026-10-13T07:58:10Z") },
            ],
        }],
        node: NodeSeen {
            version: "1.0.0".into(),
            started: at("2026-10-12T22:01:00Z"),
            service: true,
            stuck: Vec::new(),
            health: Vec::new(),
            refused: None,
        },
        jobs: vec![
            AccountJobs { account: None, running: 3, most: None },
            AccountJobs {
                account: Some(fastctx_types::access::HubAccount {
                    hub: "w-0123456789abcdef".parse().unwrap(),
                    account: "bo".parse().unwrap(),
                }),
                running: 2,
                most: Some(4),
            },
        ],
        limits: Vec::new(),
    };
    let linux = MachineStats {
        run: None,
        machine: "server".parse().unwrap(),
        at: at("2026-10-13T08:30:02Z"),
        boot: Some(at("2026-09-01T00:00:00Z")),
        system: "Ubuntu 24.04.1 LTS".into(),
        kernel: Some("Linux 6.8.0-45-generic".into()),
        host: "server".into(),
        arch: "x86_64".into(),
        cpu: Cpu { model: String::new(), threads: 2, busy: None, load: Some([0.12, 0.08, 0.05]) },
        memory: Memory { total: 2_065_588_224, used: 912_000_000, swap_total: 0, swap_used: 0, room: None, waiting: 0 },
        disks: vec![Disk {
            at: "/".into(),
            label: None,
            fs: "ext4".into(),
            total: 42_000_000_000,
            free: 18_000_000_000,
        }],
        cards: vec![],
        gpus: vec![],
        lines: vec![Line {
            kept: Some(fastctx_types::stats::LineKept { trips: 41, changes: 3 }),
            hub: "home".parse().unwrap(),
            since: at("2026-10-13T08:00:00Z"),
            up: false,
            through: Some("desktop".into()),
            trips: vec![],
            changes: vec![],
        }],
        node: NodeSeen {
            version: "1.0.0".into(),
            started: at("2026-10-01T00:00:00Z"),
            service: false,
            stuck: Vec::new(),
            health: Vec::new(),
            refused: Some(fastctx_types::stats::Refused {
                path: "/Users/me/Desktop".into(),
                at: at("2026-10-01T08:00:00Z"),
            }),
        },
        jobs: Vec::new(),
        limits: vec![LimitReading {
            at: at("2026-10-29T06:12:00Z"),
            uses: vec![
                LimitUse { kind: LimitKind::InotifyInstances, used: 107, limit: 128 },
                LimitUse { kind: LimitKind::InotifyWatches, used: 9_211, limit: 524_288 },
                LimitUse { kind: LimitKind::OpenFiles, used: 412, limit: 65_536 },
            ],
        }],
    };
    Fixtures { stats: vec![windows, linux] }
}
