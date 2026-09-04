//! What this machine tells the World about itself: hardware, disks, GPUs, shell, addresses.
//! Facts that cannot be measured are left absent rather than invented.

use crate::world::client::{Disk, Gpu, Inventory, Shell, WorldClient};
use crate::world::link::netpath;
use std::process::Command;

/// Collects the inventory; slow probes (GPU, shell) run off the async runtime.
pub(crate) async fn collect(client: &WorldClient) -> Inventory {
    let tags = client.own_tags();
    let home = client
        .paths
        .dir
        .parent()
        .map(|path| path.to_path_buf())
        .and_then(|path| path.parent().map(|path| path.to_path_buf()));
    tokio::task::spawn_blocking(move || collect_blocking(tags, home))
        .await
        .unwrap_or_default()
}

fn collect_blocking(tags: Vec<String>, home: Option<std::path::PathBuf>) -> Inventory {
    let (shell, capabilities) = probe_shell(home.as_deref());
    Inventory {
        hostname: hostname(),
        os: match std::env::consts::OS {
            "macos" => "macos",
            "windows" => "windows",
            _ => "linux",
        }
        .to_string(),
        arch: match std::env::consts::ARCH {
            "aarch64" => "aarch64",
            _ => "x86_64",
        }
        .to_string(),
        cpus: std::thread::available_parallelism()
            .map(|count| count.get() as u32)
            .unwrap_or(0),
        memory_gb: memory_gb(),
        disks: disks(home.as_deref()),
        gpus: gpus(),
        wsl2: wsl2(),
        shell,
        capabilities,
        tags,
        addresses: addresses(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        collected_at: crate::world::now_rfc3339(),
    }
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|name| !name.is_empty())
        .or_else(|| {
            Command::new("hostname")
                .output()
                .ok()
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_default()
}

fn gb(bytes: u64) -> f32 {
    ((bytes as f64) / (1024.0 * 1024.0 * 1024.0) * 10.0).round() as f32 / 10.0
}

#[cfg(windows)]
fn memory_gb() -> f32 {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return 0.0;
    }
    gb(status.ullTotalPhys)
}

#[cfg(unix)]
fn memory_gb() -> f32 {
    if cfg!(target_os = "linux") {
        if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("MemTotal:") {
                    let kib = rest
                        .trim()
                        .split_whitespace()
                        .next()
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or(0);
                    return gb(kib * 1024);
                }
            }
        }
        return 0.0;
    }
    Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<u64>()
                .ok()
        })
        .map(gb)
        .unwrap_or(0.0)
}

#[cfg(not(any(unix, windows)))]
fn memory_gb() -> f32 {
    0.0
}

fn disks(home: Option<&std::path::Path>) -> Vec<Disk> {
    let Some(home) = home else {
        return Vec::new();
    };
    let mount = if cfg!(windows) {
        home.components()
            .next()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_else(|| home.to_string_lossy().into_owned())
    } else {
        "/".to_string()
    };
    match free_space(home) {
        Some((free, total)) => vec![Disk {
            mount,
            free_gb: gb(free),
            total_gb: gb(total),
        }],
        None => Vec::new(),
    }
}

#[cfg(windows)]
fn free_space(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free_to_caller: u64 = 0;
    let mut total: u64 = 0;
    let mut free: u64 = 0;
    let ok =
        unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free_to_caller, &mut total, &mut free) };
    (ok != 0).then_some((free_to_caller, total))
}

#[cfg(unix)]
fn free_space(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    let fragment = stats.f_frsize as u64;
    Some((
        stats.f_bavail as u64 * fragment,
        stats.f_blocks as u64 * fragment,
    ))
}

#[cfg(not(any(unix, windows)))]
fn free_space(_path: &std::path::Path) -> Option<(u64, u64)> {
    None
}

fn gpus() -> Vec<Gpu> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=index,name,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
            if fields.len() < 3 {
                return None;
            }
            Some(Gpu {
                index: fields[0].parse().ok()?,
                vendor: "nvidia".to_string(),
                model: fields[1].to_string(),
                memory_gb: fields[2]
                    .parse::<f32>()
                    .ok()
                    .map(|mib| (mib / 1024.0 * 10.0).round() / 10.0)?,
            })
        })
        .collect()
}

fn wsl2() -> Option<bool> {
    if !cfg!(windows) {
        return None;
    }
    let output = Command::new("wsl.exe").args(["-l", "-q"]).output().ok()?;
    let text = String::from_utf16_lossy(
        &output
            .stdout
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    );
    Some(output.status.success() && text.lines().any(|line| !line.trim().is_empty()))
}

fn probe_shell(home: Option<&std::path::Path>) -> (Shell, Vec<String>) {
    let environment = crate::session::SessionEnvironment::capture().unwrap_or_else(|_| {
        crate::session::SessionEnvironment::new(
            home.map(|path| path.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from(".")),
            std::env::vars_os().collect(),
        )
    });
    let locator = crate::shell::bash::BashLocator::default();
    let mut capabilities = vec!["files".to_string()];
    match locator.resolve(&environment) {
        Ok(path) => {
            let login = Command::new(&path)
                .args(["-lc", "echo fastctx-login-ok"])
                .output()
                .map(|output| {
                    output.status.success()
                        && String::from_utf8_lossy(&output.stdout).contains("fastctx-login-ok")
                })
                .unwrap_or(false);
            if login {
                capabilities.push("shell".to_string());
            }
            (
                Shell {
                    kind: "bash".to_string(),
                    path: path.to_string_lossy().into_owned(),
                    login_ok: login,
                    error: (!login).then_some("bash -lc did not run cleanly".to_string()),
                },
                capabilities,
            )
        }
        Err(error) => (
            Shell {
                kind: "bash".to_string(),
                path: String::new(),
                login_ok: false,
                error: Some(error),
            },
            capabilities,
        ),
    }
}

/// Private and VPN addresses other members might reach directly; fake-IP tunnels excluded.
fn addresses() -> Vec<String> {
    let Ok(view) = netpath::scan() else {
        return Vec::new();
    };
    let mut addresses = Vec::new();
    for interface in &view.interfaces {
        for address in &interface.ipv4 {
            if netpath::is_fake_ip(std::net::IpAddr::V4(*address)) {
                continue;
            }
            addresses.push(address.to_string());
        }
    }
    addresses.sort();
    addresses.dedup();
    addresses
}
