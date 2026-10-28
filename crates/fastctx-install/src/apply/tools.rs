//! Verify the host's resulting configuration without creating an agent session.

use std::path::Path;

use super::Places;
use crate::transaction::Snapshot;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ToolCheck {
    pub host: &'static str,
    /// The observation's scope, not a promise about already-open sessions.
    pub scope: String,
    pub result: Result<(), String>,
}

impl<'de> serde::Deserialize<'de> for ToolCheck {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Wire {
            host: String,
            scope: String,
            result: Result<(), String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let host = match wire.host.as_str() {
            "Codex" => "Codex",
            "Claude Code" => "Claude Code",
            _ => return Err(serde::de::Error::custom("unknown host in the kernel's tool check")),
        };
        Ok(Self { host, scope: wire.scope, result: wire.result })
    }
}

pub(super) fn codex(places: &Places) -> ToolCheck {
    let cwd = std::env::current_dir();
    let scope = match &cwd {
        Ok(cwd) => {
            format!(
                "effective Codex features from {}, in {}; native plan flag in user config; plan overrides and explicit --profile or session/CLI overrides are not checked",
                places.codex_home.display(),
                cwd.display()
            )
        }
        Err(_) => format!("effective Codex features from {}", places.codex_home.display()),
    };
    let result = cwd.map_err(|error| error.to_string()).and_then(|cwd| codex_in(&places.codex_home, &cwd));
    ToolCheck { host: "Codex", scope, result }
}

/// What decides the answer of `codex features list` here, as far as files
/// show it: the program, the user's configuration and this directory's.
type Inputs = (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf, Vec<Option<(std::time::SystemTime, u64)>>);

/// The last answer, kept while its inputs stand: an interface asks every few
/// seconds, and each answer starts a Codex.
static LAST: std::sync::Mutex<Option<(Inputs, Result<(), String>)>> = std::sync::Mutex::new(None);

fn codex_in(home: &Path, cwd: &Path) -> Result<(), String> {
    // The program the World starts for this login: a home the desktop app
    // does not use runs the app's program the default home records.
    let environment: Vec<_> = std::env::vars_os().collect();
    let default =
        fastctx_platform::user_home().ok().map(|user| fastctx_hosts::codex::launch::codex_home(&user, &environment));
    let launch =
        fastctx_hosts::codex::launch::resolve_for(home, default.as_deref(), std::env::var_os("PATH").as_ref())?;
    let stamp = |path: &Path| {
        std::fs::metadata(path).ok().map(|meta| (meta.modified().unwrap_or(std::time::UNIX_EPOCH), meta.len()))
    };
    let inputs: Inputs = (
        home.to_path_buf(),
        launch.binary.clone(),
        cwd.to_path_buf(),
        vec![stamp(&launch.binary), stamp(&home.join("config.toml")), stamp(&cwd.join(".codex").join("config.toml"))],
    );
    if let Some((last, answer)) = LAST.lock().expect("never poisoned").as_ref()
        && *last == inputs
    {
        return answer.clone();
    }
    let answer = features(&launch, home, cwd);
    *LAST.lock().expect("never poisoned") = Some((inputs, answer.clone()));
    answer
}

fn features(launch: &fastctx_hosts::codex::launch::Launch, home: &Path, cwd: &Path) -> Result<(), String> {
    let configuration = Snapshot::read(home.join("config.toml")).map_err(|error| error.to_string())?;
    if !crate::codex::tools::plan_configured(configuration.bytes().unwrap_or_default())
        .map_err(|error| error.to_string())?
    {
        return Err("tools.update_plan.enabled is not false in the user configuration".into());
    }
    let mut environment: Vec<_> =
        std::env::vars_os().filter(|(name, _)| !name.eq_ignore_ascii_case("CODEX_HOME")).collect();
    environment.push(("CODEX_HOME".into(), home.as_os_str().to_owned()));
    // Use the host's configuration loader: reading two TOML leaves misses
    // project and managed configuration precedence.
    let answer = fastctx_hosts::agents::output(&launch.binary, &["features", "list"], cwd, &environment)?;
    if !answer.status.success() {
        // Host diagnostics can quote arbitrary settings. Do not expose them.
        return Err(format!("codex features list exited {}; inspect the configuration with Codex", answer.status));
    }
    let text = std::str::from_utf8(&answer.stdout).map_err(|_| "codex features list returned non-UTF-8 output")?;
    let mut enabled = Vec::new();
    for feature in crate::codex::tools::FEATURES {
        let rows: Vec<_> = text
            .lines()
            .map(|line| line.split_whitespace().collect::<Vec<_>>())
            .filter(|row| row.first() == Some(&feature))
            .collect();
        match rows.as_slice() {
            [row] if row.len() >= 3 && row.last() == Some(&"false") => {}
            [row] if row.len() >= 3 && row.last() == Some(&"true") => enabled.push(feature),
            _ => return Err(format!("codex features list did not report a unique boolean for {feature}")),
        }
    }
    if enabled.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "still enabled: {}; check the active profile, project configuration and managed requirements",
            enabled.join(", ")
        ))
    }
}

pub(super) fn claude(places: &Places) -> ToolCheck {
    let path = places.claude_home.join("settings.json");
    let result = Snapshot::read(&path).map_err(|error| error.to_string()).and_then(|file| {
        if crate::claude::tools::configured(file.bytes())? {
            Ok(())
        } else {
            Err(format!("permissions.deny does not contain all of {}", crate::claude::tools::DENIED.join(", ")))
        }
    });
    ToolCheck {
        host: "Claude Code",
        scope: format!("user deny rules in {}; managed policy and session overrides are not checked", path.display()),
        result,
    }
}
