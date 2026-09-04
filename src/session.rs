//! Immutable per-connection working directory, environment, and output policy.

use crate::budget::{
    GLOB_TOKEN_BUDGET_ENV, GLOBAL_TOKEN_BUDGET_ENV, GREP_TOKEN_BUDGET_ENV,
    JOB_OUTPUT_TOKEN_BUDGET_ENV, READ_TOKEN_BUDGET_ENV, RUN_TOKEN_BUDGET_ENV,
};
use crate::control::paths::{CodexHomeSource, ControlPaths};
use crate::control::provider::{EffectiveOutput, EffectiveOutputMode, ProviderDetection};
use crate::control::settings::{FastCtxSettings, ToolBudgets};
use std::cell::RefCell;
use std::env::VarError;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

thread_local! {
    static ACTIVE_ENVIRONMENT: RefCell<Option<Arc<SessionEnvironment>>> = const { RefCell::new(None) };
}

/// The native process state captured by one stdio proxy before it connects.
///
/// Environment values remain `OsString`s so Unix bytes and Windows unpaired UTF-16 units survive
/// the IPC handshake without lossy conversion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionEnvironment {
    cwd: PathBuf,
    variables: Arc<Vec<(OsString, OsString)>>,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct SessionEnvironmentWire {
    cwd: NativeOsString,
    variables: Vec<(NativeOsString, NativeOsString)>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(transparent)]
struct NativeOsString(String);

impl serde::Serialize for SessionEnvironment {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        SessionEnvironmentWire {
            cwd: NativeOsString::encode(self.cwd.as_os_str()),
            variables: self
                .variables
                .iter()
                .map(|(name, value)| (NativeOsString::encode(name), NativeOsString::encode(value)))
                .collect(),
        }
        .serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for SessionEnvironment {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = SessionEnvironmentWire::deserialize(deserializer)?;
        let cwd = PathBuf::from(wire.cwd.decode().map_err(serde::de::Error::custom)?);
        let variables = wire
            .variables
            .into_iter()
            .map(|(name, value)| {
                Ok((
                    name.decode().map_err(serde::de::Error::custom)?,
                    value.decode().map_err(serde::de::Error::custom)?,
                ))
            })
            .collect::<Result<Vec<_>, D::Error>>()?;
        Ok(Self::new(cwd, variables))
    }
}

impl NativeOsString {
    fn encode(value: &OsStr) -> Self {
        use base64::Engine;
        Self(base64::engine::general_purpose::STANDARD_NO_PAD.encode(native_bytes(value)))
    }

    fn decode(self) -> Result<OsString, String> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(self.0)
            .map_err(|error| format!("invalid native environment encoding: {error}"))?;
        native_string(bytes)
    }
}

#[cfg(unix)]
pub(crate) fn native_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().to_vec()
}

#[cfg(windows)]
pub(crate) fn native_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    value
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>()
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn native_bytes(value: &OsStr) -> Vec<u8> {
    value.to_string_lossy().as_bytes().to_vec()
}

#[cfg(unix)]
fn native_string(bytes: Vec<u8>) -> Result<OsString, String> {
    use std::os::unix::ffi::OsStringExt;
    Ok(OsString::from_vec(bytes))
}

#[cfg(windows)]
fn native_string(bytes: Vec<u8>) -> Result<OsString, String> {
    use std::os::windows::ffi::OsStringExt;
    if !bytes.len().is_multiple_of(2) {
        return Err("invalid odd-length native Windows environment value".to_string());
    }
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect::<Vec<_>>();
    Ok(OsString::from_wide(&units))
}

#[cfg(not(any(unix, windows)))]
fn native_string(bytes: Vec<u8>) -> Result<OsString, String> {
    String::from_utf8(bytes)
        .map(OsString::from)
        .map_err(|error| format!("invalid native environment UTF-8: {error}"))
}

impl SessionEnvironment {
    /// Captures the current process state without reading any configuration file.
    pub fn capture() -> Result<Self, String> {
        let cwd = std::env::current_dir()
            .map_err(|error| format!("Cannot determine the session working directory: {error}."))?;
        Ok(Self::new(cwd, std::env::vars_os().collect()))
    }

    /// Builds an immutable snapshot from native values received over IPC.
    pub fn new(cwd: PathBuf, variables: Vec<(OsString, OsString)>) -> Self {
        Self {
            cwd,
            variables: Arc::new(variables),
        }
    }

    /// The exact working directory captured by the stdio proxy.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The exact native environment captured by the stdio proxy.
    pub fn variables(&self) -> &[(OsString, OsString)] {
        self.variables.as_slice()
    }

    /// Looks up one environment value using the platform's variable-name rules.
    pub fn var_os(&self, name: &str) -> Option<OsString> {
        self.variables
            .iter()
            .rev()
            .find(|(candidate, _)| environment_name_eq(candidate, name))
            .map(|(_, value)| value.clone())
    }

    /// Looks up one UTF-8 environment value with `std::env::var`-compatible failures.
    pub fn var(&self, name: &str) -> Result<String, VarError> {
        match self.var_os(name) {
            Some(value) => value.into_string().map_err(VarError::NotUnicode),
            None => Err(VarError::NotPresent),
        }
    }

    /// Replaces a child command's inherited environment with this exact snapshot.
    pub fn configure_command(&self, command: &mut Command) {
        command.env_clear().envs(self.variables.iter().cloned());
    }

    /// Runs synchronous work with legacy budget/path helpers bound to this connection.
    pub(crate) fn activate<R>(self: &Arc<Self>, operation: impl FnOnce() -> R) -> R {
        let _scope = SessionEnvironmentScope::install(Arc::clone(self));
        operation()
    }

    fn with_guarded_output(&self, output: EffectiveOutput) -> Self {
        if output.mode != EffectiveOutputMode::Guarded {
            return self.clone();
        }
        let mut variables = self
            .variables
            .iter()
            .filter(|(name, _)| !is_budget_variable(name))
            .cloned()
            .collect::<Vec<_>>();
        variables.push((
            OsString::from(GLOBAL_TOKEN_BUDGET_ENV),
            OsString::from(output.fastctx_budget.to_string()),
        ));
        append_tool_budget(
            &mut variables,
            READ_TOKEN_BUDGET_ENV,
            output.tool_budgets.read.resolve(output.fastctx_budget),
        );
        append_tool_budget(
            &mut variables,
            GREP_TOKEN_BUDGET_ENV,
            output.tool_budgets.grep.resolve(output.fastctx_budget),
        );
        append_tool_budget(
            &mut variables,
            GLOB_TOKEN_BUDGET_ENV,
            output.tool_budgets.glob.resolve(output.fastctx_budget),
        );
        append_tool_budget(
            &mut variables,
            RUN_TOKEN_BUDGET_ENV,
            output.tool_budgets.run.resolve(output.fastctx_budget),
        );
        append_tool_budget(
            &mut variables,
            JOB_OUTPUT_TOKEN_BUDGET_ENV,
            output
                .tool_budgets
                .job_output
                .resolve(output.fastctx_budget),
        );
        Self::new(self.cwd.clone(), variables)
    }
}

/// All immutable state derived once when a control-center connection is accepted.
#[derive(Clone, Debug)]
pub struct SessionContext {
    /// Exact native cwd/env captured by this connection's stdio proxy.
    ///
    /// This stays the sole basis for FastCtx's own identity — control paths, endpoint, budgets —
    /// so restoring the machine's persisted environment can never relocate a user's state.
    pub environment: Arc<SessionEnvironment>,
    /// Session state used only by FastCtx's internal path and response-budget helpers.
    tool_environment: Arc<SessionEnvironment>,
    /// Environment handed to commands the user runs, with the machine's persisted values restored.
    pub command_environment: Arc<SessionEnvironment>,
    /// Per-user paths resolved exclusively from the connection environment.
    pub control_paths: ControlPaths,
    /// Saved FastCtx preferences visible to this connection.
    pub settings: FastCtxSettings,
    /// Provider classification made for this connection.
    pub provider: ProviderDetection,
    /// Provider-aware output policy made for this connection.
    pub effective_output: EffectiveOutput,
}

impl SessionContext {
    /// Captures and resolves one standalone server process as a single connection.
    pub fn capture() -> Result<Arc<Self>, String> {
        Self::from_environment(SessionEnvironment::capture()?)
    }

    /// Resolves per-user paths, settings, provider, and runtime budgets from one handshake.
    pub fn from_environment(environment: SessionEnvironment) -> Result<Arc<Self>, String> {
        let control_paths = control_paths_from_environment(&environment)?;
        let settings = crate::control::settings::load(&control_paths)?;
        let provider = crate::control::provider::detect_path(&control_paths.codex_config);
        let effective_output = crate::control::provider::effective_output(
            settings.tier,
            settings.tool_budgets,
            settings.output_guard.enabled,
            &provider,
        );
        let tool_environment = Arc::new(environment.with_guarded_output(effective_output));
        let command_environment =
            Arc::new(crate::os_environment::command_environment(&environment));
        let environment = Arc::new(environment);
        Ok(Arc::new(Self {
            environment,
            tool_environment,
            command_environment,
            control_paths,
            settings,
            provider,
            effective_output,
        }))
    }

    /// Best-effort context used only by direct library constructors that cannot return startup errors.
    pub(crate) fn library_default() -> Arc<Self> {
        Self::capture().unwrap_or_else(|_| {
            let environment = SessionEnvironment::capture().unwrap_or_else(|_| {
                SessionEnvironment::new(PathBuf::from("."), std::env::vars_os().collect())
            });
            let home = environment
                .var_os("HOME")
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    environment
                        .var_os("USERPROFILE")
                        .filter(|value| !value.is_empty())
                })
                .map(PathBuf::from)
                .unwrap_or_else(|| environment.cwd().to_path_buf());
            let control_paths = ControlPaths::for_home(home);
            let settings = FastCtxSettings::default();
            let provider = crate::control::provider::detect_path(&control_paths.codex_config);
            let effective_output = crate::control::provider::effective_output(
                settings.tier,
                settings.tool_budgets,
                settings.output_guard.enabled,
                &provider,
            );
            let tool_environment = Arc::new(environment.with_guarded_output(effective_output));
            let command_environment =
                Arc::new(crate::os_environment::command_environment(&environment));
            Arc::new(Self {
                tool_environment,
                command_environment,
                environment: Arc::new(environment),
                control_paths,
                settings,
                provider,
                effective_output,
            })
        })
    }

    /// Derives the context a World call from another member runs under on this machine: this
    /// machine's own environment and settings, the caller's working directory, and the
    /// caller's response budgets in place of whatever this machine's host would have set.
    pub(crate) fn for_remote_call(
        base: &SessionContext,
        cwd: PathBuf,
        budget_overrides: Vec<(String, String)>,
    ) -> Arc<Self> {
        let mut variables = base
            .environment
            .variables()
            .iter()
            .filter(|(name, _)| !is_budget_variable(name))
            .cloned()
            .collect::<Vec<_>>();
        variables.extend(
            budget_overrides
                .into_iter()
                .map(|(name, value)| (OsString::from(name), OsString::from(value))),
        );
        let environment = SessionEnvironment::new(cwd.clone(), variables);
        let command_environment = Arc::new(SessionEnvironment::new(
            cwd,
            base.command_environment.variables().to_vec(),
        ));
        Arc::new(Self {
            tool_environment: Arc::new(environment.clone()),
            environment: Arc::new(environment),
            command_environment,
            control_paths: base.control_paths.clone(),
            settings: base.settings.clone(),
            provider: base.provider.clone(),
            effective_output: base.effective_output,
        })
    }

    /// Runs synchronous FastCtx work with this connection's cwd and effective response budgets.
    pub(crate) fn activate<R>(&self, operation: impl FnOnce() -> R) -> R {
        self.tool_environment.activate(operation)
    }

    /// Effective per-tool shares retained for diagnostics and tests.
    pub fn tool_budgets(&self) -> ToolBudgets {
        self.effective_output.tool_budgets
    }
}

struct SessionEnvironmentScope {
    previous: Option<Arc<SessionEnvironment>>,
}

impl SessionEnvironmentScope {
    fn install(environment: Arc<SessionEnvironment>) -> Self {
        let previous = ACTIVE_ENVIRONMENT.with(|slot| slot.borrow_mut().replace(environment));
        Self { previous }
    }
}

impl Drop for SessionEnvironmentScope {
    fn drop(&mut self) {
        ACTIVE_ENVIRONMENT.with(|slot| *slot.borrow_mut() = self.previous.take());
    }
}

/// Clones the active connection environment for work that must cross a thread boundary.
#[cfg(feature = "pdf")]
pub(crate) fn active_environment() -> Option<Arc<SessionEnvironment>> {
    ACTIVE_ENVIRONMENT.with(|slot| slot.borrow().clone())
}

/// Returns the active connection cwd, falling back only for non-daemon library/control callers.
pub(crate) fn current_dir() -> std::io::Result<PathBuf> {
    ACTIVE_ENVIRONMENT.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|environment| environment.cwd.clone())
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
    })
}

/// Returns an active native environment value, falling back only outside a tool request.
#[cfg(feature = "pdf")]
pub(crate) fn var_os(name: &str) -> Option<OsString> {
    ACTIVE_ENVIRONMENT.with(|slot| match slot.borrow().as_ref() {
        Some(environment) => environment.var_os(name),
        None => std::env::var_os(name),
    })
}

/// Returns an active connection variable, falling back only outside a tool request.
pub(crate) fn var(name: &str) -> Result<String, VarError> {
    ACTIVE_ENVIRONMENT.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|environment| environment.var(name))
            .unwrap_or_else(|| std::env::var(name))
    })
}

/// Resolves a temporary directory from the active session without consulting daemon state.
#[cfg(feature = "pdf")]
pub(crate) fn temp_dir() -> PathBuf {
    let Some(environment) = active_environment() else {
        return std::env::temp_dir();
    };
    let names: &[&str] = if cfg!(windows) {
        &["TMP", "TEMP"]
    } else {
        &["TMPDIR", "TMP", "TEMP"]
    };
    if let Some(path) = names
        .iter()
        .filter_map(|name| environment.var_os(name))
        .find(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return if path.is_absolute() {
            path
        } else {
            environment.cwd().join(path)
        };
    }
    #[cfg(windows)]
    {
        environment
            .var_os("USERPROFILE")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join("AppData/Local/Temp"))
            .unwrap_or_else(|| environment.cwd().join(".fastctx-tmp"))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/tmp")
    }
}

fn control_paths_from_environment(
    environment: &SessionEnvironment,
) -> Result<ControlPaths, String> {
    let home = environment
        .var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| {
            environment
                .var_os("USERPROFILE")
                .filter(|value| !value.is_empty())
        })
        .map(PathBuf::from)
        .ok_or_else(|| {
            "Cannot determine the user home directory. Set HOME or USERPROFILE and retry."
                .to_string()
        })?;
    let (codex_home, source) = match environment
        .var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
    {
        Some(path) => (PathBuf::from(path), CodexHomeSource::Environment),
        None => (home.join(".codex"), CodexHomeSource::Default),
    };
    Ok(ControlPaths::for_home_and_codex_home(
        home, codex_home, source,
    ))
}

fn append_tool_budget(
    variables: &mut Vec<(OsString, OsString)>,
    name: &'static str,
    value: Option<usize>,
) {
    if let Some(value) = value {
        variables.push((OsString::from(name), OsString::from(value.to_string())));
    }
}

fn is_budget_variable(name: &OsStr) -> bool {
    [
        GLOBAL_TOKEN_BUDGET_ENV,
        READ_TOKEN_BUDGET_ENV,
        GREP_TOKEN_BUDGET_ENV,
        GLOB_TOKEN_BUDGET_ENV,
        RUN_TOKEN_BUDGET_ENV,
        JOB_OUTPUT_TOKEN_BUDGET_ENV,
    ]
    .iter()
    .any(|candidate| environment_name_eq(name, candidate))
}

#[cfg(windows)]
pub(crate) fn environment_name_eq(candidate: &OsStr, expected: &str) -> bool {
    candidate
        .to_str()
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(expected))
}

#[cfg(not(windows))]
pub(crate) fn environment_name_eq(candidate: &OsStr, expected: &str) -> bool {
    candidate == OsStr::new(expected)
}

/// Compares two native variable names under the same rules `environment_name_eq` applies.
pub(crate) fn environment_name_eq_os(candidate: &OsStr, expected: &OsStr) -> bool {
    match expected.to_str() {
        Some(expected) => environment_name_eq(candidate, expected),
        None => candidate == expected,
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionContext, SessionEnvironment};
    use crate::budget::{GLOBAL_TOKEN_BUDGET_ENV, GREP_TOKEN_BUDGET_ENV, tool_token_budget};
    use crate::control::provider::EffectiveOutputMode;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn guarded_session_overlays_only_budget_variables_and_preserves_native_state() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let codex = home.join(".codex");
        std::fs::create_dir_all(&codex).unwrap();
        std::fs::write(
            codex.join("config.toml"),
            "model_provider='third'\n[model_providers.third]\nname='Third Party'\n",
        )
        .unwrap();
        let environment = SessionEnvironment::new(
            temp.path().to_path_buf(),
            vec![
                (OsString::from("HOME"), home.into_os_string()),
                (
                    OsString::from(GLOBAL_TOKEN_BUDGET_ENV),
                    OsString::from("54000"),
                ),
                (
                    OsString::from(GREP_TOKEN_BUDGET_ENV),
                    OsString::from("10800"),
                ),
                (OsString::from("SESSION_SENTINEL"), OsString::from("kept")),
            ],
        );

        let session = SessionContext::from_environment(environment).unwrap();
        assert_eq!(session.effective_output.mode, EffectiveOutputMode::Guarded);
        assert_eq!(
            session.environment.var(GLOBAL_TOKEN_BUDGET_ENV).unwrap(),
            "54000"
        );
        assert_eq!(
            session.environment.var(GREP_TOKEN_BUDGET_ENV).unwrap(),
            "10800"
        );
        assert_eq!(session.environment.var("SESSION_SENTINEL").unwrap(), "kept");
        session.activate(|| {
            assert_eq!(
                tool_token_budget(GREP_TOKEN_BUDGET_ENV).unwrap().value,
                9_000
            );
        });
    }

    #[test]
    fn nested_activation_restores_the_outer_connection() {
        let first = Arc::new(SessionEnvironment::new(
            PathBuf::from("first"),
            vec![(OsString::from("MARK"), OsString::from("first"))],
        ));
        let second = Arc::new(SessionEnvironment::new(
            PathBuf::from("second"),
            vec![(OsString::from("MARK"), OsString::from("second"))],
        ));
        first.activate(|| {
            assert_eq!(super::var("MARK").unwrap(), "first");
            second.activate(|| assert_eq!(super::var("MARK").unwrap(), "second"));
            assert_eq!(super::var("MARK").unwrap(), "first");
        });
    }

    #[test]
    fn environment_json_round_trip_preserves_native_values() {
        let environment = SessionEnvironment::new(
            PathBuf::from("workspace"),
            vec![(OsString::from("NAME"), OsString::from("value"))],
        );
        let encoded = serde_json::to_vec(&environment).unwrap();
        let decoded: SessionEnvironment = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, environment);
    }
}
