//! A separate SYSTEM task checks the SCM guardian once per minute. It can
//! start only the fixed service id; its binary and definition are admin-owned.

use std::path::PathBuf;
use std::process::Command;

use anyhow::Context as _;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use super::super::{RecoveryStatus, checked};

#[derive(Deserialize, Serialize)]
pub(super) struct Snapshot {
    xml: Option<String>,
    enabled: bool,
    state: u32,
    security: Option<String>,
}

pub(super) struct Task {
    name: String,
    marker: String,
}

impl Task {
    pub fn new(service: &str) -> Self {
        Self { name: format!("{service}-check"), marker: format!("fastctx guardian check v1: {service}") }
    }
    fn prefix(&self) -> String {
        format!(
            r#"
$scheduler=New-Object -ComObject 'Schedule.Service'
$scheduler.Connect()
$folder=$scheduler.GetFolder('\')
$task=$null
try {{$task=$folder.GetTask('{}')}} catch {{if($_.Exception.HResult -ne -2147024894){{throw}}}}
if($null -ne $task -and $task.Definition.RegistrationInfo.Description -ne '{}'){{throw 'This task is not the owned guardian check.'}}
"#,
            self.name, self.marker
        )
    }
    pub fn capture(&self) -> anyhow::Result<Snapshot> {
        let json = powershell(&format!(
            r#"{}
if($null -eq $task){{$result=@{{xml=$null;enabled=$false;state=0;security=$null}}}}else{{$result=@{{xml=$task.Xml;enabled=[bool]$task.Enabled;state=[int]$task.State;security=$task.GetSecurityDescriptor(7)}}}}
[Console]::Write(($result|ConvertTo-Json -Compress))
"#,
            self.prefix()
        ))?;
        Ok(serde_json::from_str(&json)?)
    }
    pub fn status(&self) -> anyhow::Result<RecoveryStatus> {
        let value = self.capture()?;
        Ok(RecoveryStatus {
            name: self.name.clone(),
            enabled: value.enabled,
            state: if value.xml.is_none() {
                "absent"
            } else {
                match value.state {
                    1 => "disabled",
                    2 => "queued",
                    3 => "ready",
                    4 => "running",
                    _ => "unknown",
                }
            }
            .into(),
        })
    }
    pub fn install(&self, service: &str, user: Option<&str>) -> anyhow::Result<()> {
        let user = user.map(|sid| format!("(A;;GRGX;;;{sid})")).unwrap_or_default();
        powershell(&format!(
            r#"{}
$d=$scheduler.NewTask(0)
$d.RegistrationInfo.Description='{}'
$d.Principal.UserId='S-1-5-18';$d.Principal.LogonType=5
$t=$d.Triggers.Create(2)
$t.StartBoundary=[DateTime]::UtcNow.Date.ToString("yyyy-MM-ddTHH:mm:ss'Z'")
$t.DaysInterval=1
$t.Repetition.Interval='PT1M'
$t.Repetition.Duration='P1D'
$t.Repetition.StopAtDurationEnd=$false
$boot=$d.Triggers.Create(8);$boot.Enabled=$true
$d.Settings.Enabled=$true
$d.Settings.MultipleInstances=2
$d.Settings.DisallowStartIfOnBatteries=$false
$d.Settings.StopIfGoingOnBatteries=$false
$d.Settings.ExecutionTimeLimit='PT0S'
$d.Settings.StartWhenAvailable=$true
$d.Settings.RunOnlyIfNetworkAvailable=$false
$d.Settings.RunOnlyIfIdle=$false
$a=$d.Actions.Create(0)
$a.Path='%ProgramData%\fastctx-services\{service}\guardian.exe'
$a.Arguments='service-check {service}'
$null=$folder.RegisterTaskDefinition('{}',$d,6,'S-1-5-18',$null,5,'O:BAG:BAD:P(A;;GA;;;SY)(A;;GA;;;BA){user}')
"#,
            self.prefix(),
            self.marker,
            self.name
        ))?;
        Ok(())
    }
    pub fn delete(&self) -> anyhow::Result<()> {
        powershell(&format!(
            "{}\nif($null -ne $task){{$task.Enabled=$false;$task.Stop(0);$folder.DeleteTask('{}',0)}}",
            self.prefix(),
            self.name
        ))?;
        Ok(())
    }
    pub fn restore(&self, before: &Snapshot) -> anyhow::Result<()> {
        match &before.xml {
            None => self.delete(),
            Some(xml) => {
                powershell(&format!(
                    "{}\n$task=$folder.RegisterTask('{}','{}',6,'S-1-5-18',$null,5,'{}');$task.Enabled=${}",
                    self.prefix(),
                    self.name,
                    xml.replace('\'', "''"),
                    before.security.as_ref().context("checker snapshot omitted security")?.replace('\'', "''"),
                    before.enabled
                ))?;
                Ok(())
            }
        }
    }
}

fn powershell(script: &str) -> anyhow::Result<String> {
    let script = format!(
        "$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue';[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false)\ntry{{\n{script}\nexit 0}}catch{{[Console]::Error.WriteLine(($_|Out-String));exit 1}}"
    );
    let encoded = base64::engine::general_purpose::STANDARD
        .encode(script.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>());
    let root = std::env::var_os("SystemRoot").context("Windows did not supply SystemRoot")?;
    checked(Command::new(PathBuf::from(root).join("System32/WindowsPowerShell/v1.0/powershell.exe")).args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-EncodedCommand",
        &encoded,
    ]))
}
