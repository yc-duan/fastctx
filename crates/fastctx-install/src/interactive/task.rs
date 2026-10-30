//! Short-lived per-user Task Scheduler requests. Registration grants no extra
//! privilege: every action uses the same user's InteractiveToken and LUA level.

use anyhow::{Context as _, ensure};
use base64::Engine as _;
use std::path::Path;
use std::time::Duration;

fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
fn argument(value: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        result.extend(std::iter::repeat_n('\\', if ch == '"' { slashes * 2 + 1 } else { slashes }));
        result.push(ch);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    result
}
fn encoded(script: &str) -> String {
    base64::engine::general_purpose::STANDARD
        .encode(script.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>())
}

pub fn run(root: &Path, executable: &Path, args: &[String], timeout: Duration, detach: bool) -> anyhow::Result<()> {
    ensure!(executable.is_absolute(), "interactive executable is not absolute: {}", executable.display());
    let (sid, session) = super::session::current()?;
    let temporary = tempfile::Builder::new()
        .prefix("interactive-")
        .tempdir_in(root.join("updates"))
        .with_context(|| format!("preparing interactive execution under {}", root.display()))?;
    let receipt = temporary.path().join("started.json");
    let failure = temporary.path().join("failure.txt");
    let powershell = std::path::PathBuf::from(std::env::var_os("SystemRoot").context("Windows SystemRoot is missing")?)
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let joined = args.iter().map(|a| argument(a)).collect::<Vec<_>>().join(" ");
    let action = format!(
        r#"
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
try {{
  $env:FASTCTX_HOME={root}
  $start=New-Object Diagnostics.ProcessStartInfo
  $start.FileName={executable}
  $start.Arguments={arguments}
  $start.WorkingDirectory={working}
  $start.UseShellExecute=$false
  $start.CreateNoWindow=$true
  $process=[Diagnostics.Process]::Start($start)
  if ($null -eq $process) {{ throw 'process creation returned no process' }}
  [IO.File]::WriteAllText({receipt}, [string]$process.Id)
  {finish}
}} catch {{ [IO.File]::WriteAllText({failure}, $_.ToString()); exit 1 }}
"#,
        root = quoted(&root.display().to_string()),
        executable = quoted(&executable.display().to_string()),
        arguments = quoted(&joined),
        working = quoted(&executable.parent().context("executable has no parent")?.display().to_string()),
        receipt = quoted(&receipt.display().to_string()),
        failure = quoted(&failure.display().to_string()),
        finish = if detach {
            "exit 0".into()
        } else {
            format!(
                "if (-not $process.WaitForExit({})) {{ $process.Kill(); $process.WaitForExit(); throw 'interactive program exceeded its deadline' }}; exit $process.ExitCode",
                timeout.as_millis()
            )
        },
    );
    let task_name = format!(
        "Fastctx-interactive-{}-{}",
        std::process::id(),
        temporary.path().file_name().context("temporary directory has no name")?.to_string_lossy()
    );
    let script = format!(
        r#"
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$service=New-Object -ComObject Schedule.Service
$service.Connect()
$folder=$service.GetFolder('\')
$definition=$service.NewTask(0)
$definition.Principal.UserId={sid}
$definition.Principal.LogonType=3
$definition.Principal.RunLevel=0
$definition.Settings.AllowDemandStart=$true
$definition.Settings.DisallowStartIfOnBatteries=$false
$definition.Settings.StopIfGoingOnBatteries=$false
$definition.Settings.ExecutionTimeLimit='PT0S'
$definition.Settings.MultipleInstances=2
$action=$definition.Actions.Create(0)
$action.Path={powershell}
$action.Arguments={action}
$action.WorkingDirectory={working}
$registered=$false
$running=$null
$completed=$false
try {{
  $task=$folder.RegisterTaskDefinition({name},$definition,2,{sid},$null,3,$null)
  $registered=$true
  $running=$task.RunEx($null,12,{session},{sid})
  $deadline=[DateTime]::UtcNow.AddSeconds({seconds})
  do {{
    $task=$folder.GetTask({name})
    if ({detach} -and [IO.File]::Exists({receipt})) {{ $completed=$true; break }}
    if ($task.LastRunTime.Year -gt 2000 -and $task.State -notin @(2,4) -and $task.LastTaskResult -notin @(267009,267011,267045)) {{
      $completed=$true
      if ($task.LastTaskResult -ne 0) {{
        $why=if ([IO.File]::Exists({failure})) {{ [IO.File]::ReadAllText({failure}) }} else {{ '' }}
        throw ('interactive program exited '+$task.LastTaskResult+': '+$why)
      }}
      if (-not [IO.File]::Exists({receipt})) {{ throw 'interactive action exited without starting the program' }}
      $completed=$true
      break
    }}
    if ([DateTime]::UtcNow -gt $deadline) {{ throw 'interactive program exceeded its deadline' }}
    Start-Sleep -Milliseconds 100
  }} while ($true)
}} finally {{
  try {{
    if (-not $completed -and $null -ne $running) {{
      try {{ $running.Stop() }} catch {{ [Console]::Error.WriteLine('Stopping interactive task: '+$_.Exception.Message) }}
    }}
  }} finally {{ if ($registered) {{ $folder.DeleteTask({name},0) }} }}
}}
"#,
        sid = quoted(&sid),
        powershell = quoted(&powershell.display().to_string()),
        action =
            quoted(&format!("-NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand {}", encoded(&action))),
        working = quoted(&temporary.path().display().to_string()),
        name = quoted(&task_name),
        session = session,
        seconds = timeout.as_secs() + 15,
        failure = quoted(&failure.display().to_string()),
        detach = if detach { "$true" } else { "$false" },
        receipt = quoted(&receipt.display().to_string()),
    );
    let script = format!("try {{ {script} }} catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 1 }}");
    use std::os::windows::process::CommandExt as _;
    let output = std::process::Command::new(&powershell)
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded(&script)])
        .creation_flags(0x08000000)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| {
            format!("starting interactive task {task_name} for {} in session {session}", executable.display())
        })?;
    ensure!(
        output.status.success(),
        "interactive task {task_name} for {} in session {session} exited {}: {} {}",
        executable.display(),
        output.status,
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    temporary.close().with_context(|| format!("removing interactive task files for {}", executable.display()))?;
    Ok(())
}
