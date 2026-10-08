//! Exercises the production copy cleanup and job supervisor with native
//! processes. The caller supplies bash so no user profile or registry is read.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use fastctx_hands::run::{self, record, shell};
use fastctx_platform::{FileLock, ensure_private_dir, is_alive};
use fastctx_types::layout::WorldDir;
use fastctx_types::run::{EndedHow, RunRequest};
use fastctx_types::{Address, MachineName, ResidentId};
use sha2::{Digest as _, Sha256};

#[path = "../src/own.rs"]
mod own;

fn main() -> anyhow::Result<()> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match args.first().and_then(|arg| arg.to_str()) {
        Some("--version") => println!("program {}", hash(&std::env::current_exe()?)?),
        Some("pin") => {
            let world = WorldDir::at(Path::new(&args[1]));
            ensure_private_dir(world.path())?;
            let _lock = FileLock::acquire(&world.node_lock())?;
            let (copy, kept) = own::pin(&world)?;
            let kept: Vec<_> = kept.into_iter().map(|(path, error)| (path, error.to_string())).collect();
            println!("{}", serde_json::json!({"copy": copy, "kept": kept}));
        }
        Some("supervise") => {
            let fastctx = std::env::current_exe()?;
            run::supervisor::supervise_with(Path::new(&args[1]), Path::new(&args[2]), |world, job| {
                let resident = world.job_owner(&job.machine, job.town.as_ref(), &job.owner);
                let root = PathBuf::from(std::env::var_os("FASTCTX_TEST_ROOT").expect("test root"));
                let bash = PathBuf::from(std::env::var_os("FASTCTX_TEST_BASH").expect("test bash"));
                let environment = shell::environment_from(
                    environment(&root, &bash),
                    &shell::Context {
                        machine: &job.machine,
                        owner: &job.owner,
                        town: job.town.as_ref(),
                        fastctx_dir: fastctx.parent().expect("program directory"),
                        fastctx_home: Some(root.as_os_str()),
                        job: Some(&job.id),
                        scratch: None,
                        writes_off: false,
                        on_state: None,
                    },
                );
                Ok(shell::Prepared {
                    program: bash,
                    args: vec!["--noprofile".into(), "--norc".into(), script_path(&resident.job_script(&job.id))],
                    environment,
                })
            })?;
        }
        Some("wait") => {
            let mut stream = TcpStream::connect(args[1].to_str().expect("loopback address"))?;
            stream.write_all(b"ready")?;
            let mut released = [0];
            stream.read_exact(&mut released)?;
            anyhow::ensure!(released == [1]);
        }
        _ => exercise()?,
    }
    Ok(())
}

fn exercise() -> anyhow::Result<()> {
    // The bash `run` itself would start, unless the caller names another;
    // the processes under test get only the environment built below.
    let bash = match std::env::var_os("FASTCTX_TEST_BASH") {
        Some(bash) => PathBuf::from(bash),
        None => shell::bash()?,
    };
    anyhow::ensure!(bash.is_absolute() && bash.is_file());
    let base = std::env::var_os("FASTCTX_TEST_ROOT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    anyhow::ensure!(base.is_absolute() && base.is_dir());
    let root = base.join(format!("fastctx-own-lifetime-{}", std::process::id()));
    fs::create_dir(&root)?;
    let world = WorldDir::under(&root);
    ensure_private_dir(world.path())?;
    let executable = std::env::current_exe()?;
    let old = pin(&executable, &world, &root, &bash)?;
    let old_program = PathBuf::from(old["copy"].as_str().expect("old copy"));
    let old_hash = hash(&old_program)?;
    let new_program = root.join(if cfg!(windows) { "successor.exe" } else { "successor" });
    fs::copy(&executable, &new_program)?;
    // Both PE and ELF permit a trailing overlay. It gives the successor a
    // different content identity without substituting the lifecycle code.
    fs::OpenOptions::new().append(true).open(&new_program)?.write_all(b"successor")?;
    let unused = root.join(if cfg!(windows) { "unused.exe" } else { "unused" });
    fs::copy(&executable, &unused)?;
    fs::OpenOptions::new().append(true).open(&unused)?.write_all(b"unused")?;
    let unused_directory = world.path().join("bin").join(&hash(&unused)?[..24]);
    ensure_private_dir(&unused_directory)?;
    fs::copy(&unused, unused_directory.join(old_program.file_name().expect("program name")))?;

    let gate = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let address = gate.local_addr()?;
    let delayed = root.join("delayed.sh");
    let output = root.join("descendant.log");
    let status = root.join("descendant.status");
    fs::write(
        &delayed,
        format!(
            "fastctx wait '{address}'\nfastctx --version\nresult=$?\nprintf '%s\\n' \"$result\" > '{}'\nexit \"$result\"\n",
            display(&status)
        ),
    )?;
    let script = format!(
        "'{}' --noprofile --norc '{}' > '{}' 2>&1 < /dev/null &\nexit 0\n",
        display(&bash),
        display(&delayed),
        display(&output)
    );
    let machine: MachineName = "copy-test".parse()?;
    let owner = Address { machine: machine.clone(), resident: ResidentId::try_from("lab-cafe".to_string())? };
    let environment = environment(&root, &bash);
    let launched = {
        let _lock = FileLock::acquire(&world.node_lock())?;
        run::launch(&run::Launch {
            on: None,
            world: &world,
            machine: &machine,
            owner: &owner.clone().into(),
            town: None,
            request: &RunRequest { script, cwd: Some(display(&root)), writes_off: false },
            home: &root,
            executable: &old_program,
            environment: &environment,
        })
        .map_err(|error| anyhow::anyhow!("{error:?}"))?
    };
    gate.set_nonblocking(true)?;
    let mut connection = loop {
        match gate.accept() {
            Ok((connection, _)) => break connection,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let job = record::read(&launched.record)?;
                anyhow::ensure!(job.end.is_none(), "job ended before reaching the gate: {job:?}");
                let supervisor = job
                    .supervisor
                    .as_ref()
                    .or(launched.supervisor.as_ref())
                    .ok_or_else(|| anyhow::anyhow!("supervisor exited before reaching the gate"))?;
                anyhow::ensure!(is_alive(supervisor)?, "supervisor exited before reaching the gate");
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    };
    let mut ready = [0; 5];
    connection.read_exact(&mut ready)?;
    anyhow::ensure!(&ready == b"ready");
    let running = loop {
        let job = record::read(&launched.record)?;
        anyhow::ensure!(job.end.is_none(), "job ended before its descendant was released: {job:?}");
        if job.exit.is_some() && job.output_ended_at.is_some() && job.remaining.is_some_and(|count| count > 0) {
            break job;
        }
        // Observe the supervisor's publication; this does not bound how
        // long the tested program may run.
        thread::sleep(Duration::from_millis(10));
    };
    let next = pin(&new_program, &world, &root, &bash)?;
    anyhow::ensure!(old_program.is_file() && hash(&old_program)? == old_hash);
    anyhow::ensure!(!unused_directory.exists());
    anyhow::ensure!(next["kept"].as_array().expect("kept copies").len() == 1);
    connection.write_all(&[1])?;
    let ended = loop {
        let job = record::read(&launched.record)?;
        if job.end.is_some() {
            break job;
        }
        thread::sleep(Duration::from_millis(10));
    };
    anyhow::ensure!(matches!(ended.end.as_ref().map(|end| &end.how), Some(EndedHow::Finished)));
    let supervisor = ended.supervisor.as_ref().expect("supervisor identity");
    while is_alive(supervisor)? {
        thread::sleep(Duration::from_millis(10));
    }
    let text = fs::read_to_string(&output)?;
    anyhow::ensure!(text.replace('\r', "") == format!("program {old_hash}\n"), "{text}");
    anyhow::ensure!(fs::read_to_string(status)?.trim() == "0");
    let cleaned = pin(&new_program, &world, &root, &bash)?;
    anyhow::ensure!(!old_program.parent().expect("old directory").exists());
    anyhow::ensure!(cleaned["kept"].as_array().expect("kept copies").is_empty());
    println!(
        "{}",
        serde_json::json!({
            "world": world.path(), "running": running, "ended": ended,
        "during_use": next, "after_use": cleaned, "descendant_output": text, "descendant_exit": 0
        })
    );
    anyhow::ensure!(root.canonicalize()?.parent() == Some(base.canonicalize()?.as_path()));
    fs::remove_dir_all(&root)?;
    Ok(())
}

fn pin(executable: &Path, world: &WorldDir, root: &Path, bash: &Path) -> anyhow::Result<serde_json::Value> {
    let output =
        Command::new(executable).arg("pin").arg(world.path()).env_clear().envs(environment(root, bash)).output()?;
    anyhow::ensure!(output.status.success(), "pin failed: {}", String::from_utf8_lossy(&output.stderr));
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn environment(root: &Path, bash: &Path) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = [
        ("HOME", root.as_os_str()),
        ("USERPROFILE", root.as_os_str()),
        ("FASTCTX_TEST_ROOT", root.as_os_str()),
        ("FASTCTX_TEST_BASH", bash.as_os_str()),
        ("FASTCTX_HOME", root.as_os_str()),
        ("TEMP", root.as_os_str()),
        ("TMP", root.as_os_str()),
    ]
    .into_iter()
    .map(|(name, value)| (name.into(), value.to_owned()))
    .collect();
    let path = if cfg!(windows) {
        std::env::join_paths([bash.parent().expect("bash directory"), Path::new("C:/Windows/System32")]).unwrap()
    } else {
        OsString::from("/usr/bin:/bin")
    };
    env.push(("PATH".into(), path));
    if cfg!(windows) {
        env.push(("SystemRoot".into(), "C:/Windows".into()));
        env.push(("WINDIR".into(), "C:/Windows".into()));
    }
    env
}

fn display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn script_path(path: &Path) -> OsString {
    if cfg!(windows) { display(path).into() } else { path.as_os_str().to_owned() }
}

fn hash(path: &Path) -> io::Result<String> {
    let bytes = fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
