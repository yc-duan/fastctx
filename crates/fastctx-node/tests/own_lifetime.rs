//! Exercises stable program publication and job supervision with native
//! processes. The caller supplies bash and the fixture supplies its login profile.

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
use fastctx_types::layout::{Names, WorldDir};
use fastctx_types::run::{EndedHow, RunRequest};
use fastctx_types::{Address, MachineName, ResidentId};
use sha2::{Digest as _, Sha256};

#[path = "../src/world_program.rs"]
mod world_program;

fn main() -> anyhow::Result<()> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match args.first().and_then(|arg| arg.to_str()) {
        Some("--version") => println!("program {}", hash(&std::env::current_exe()?)?),
        Some("pin") => {
            let world = WorldDir::at(Path::new(&args[1]));
            let root = world.path().parent().expect("World root");
            let copy = fastctx_install::renewal::installed(root);
            ensure_private_dir(copy.parent().expect("bin directory"))?;
            fastctx_install::renewal::publish_image(&std::env::current_exe()?, &copy)?;
            let output = Command::new(&copy).arg("resources").arg(world.path()).output()?;
            anyhow::ensure!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            print!("{}", String::from_utf8(output.stdout)?);
        }
        Some("resources") => {
            let root = Path::new(&args[1]).parent().expect("World root");
            let copy = world_program::prepare(root)?;
            println!("{}", serde_json::json!({"copy": copy}));
        }
        Some("supervise") => {
            let fastctx = std::env::current_exe()?;
            run::supervisor::supervise_with(Path::new(&args[1]), Path::new(&args[2]), |world, job| {
                let resident = world.job_owner(&job.machine, &job.owner);
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
                        job_temp: None,
                        writes_off: false,
                        on_state: None,
                    },
                );
                shell::ensure_prelude(fastctx.parent().expect("program directory"))?;
                Ok(shell::Prepared {
                    program: bash,
                    args: vec!["--login".into(), script_path(&resident.job_script(&job.id))],
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
    fs::write(root.join(".bash_profile"), "export PATH=/usr/bin:/bin\nexport FASTCTX_TEST_PROFILE=yes\n")?;
    let world = WorldDir::under(&root);
    ensure_private_dir(world.path())?;
    let executable = std::env::current_exe()?;
    let cache = root.join("npm-cache");
    fs::create_dir(&cache)?;
    let cached = cache.join(if cfg!(windows) { "fastctx.exe" } else { "fastctx" });
    fs::copy(&executable, &cached)?;
    let old = pin(&cached, &world, &root, &bash)?;
    fs::remove_file(&cached)?;
    fs::remove_dir(&cache)?;
    let old_program = PathBuf::from(old["copy"].as_str().expect("old copy"));
    let new_program = root.join(if cfg!(windows) { "successor.exe" } else { "successor" });
    fs::copy(&executable, &new_program)?;
    // Both PE and ELF permit a trailing overlay. It gives the successor a
    // different content identity without substituting the lifecycle code.
    fs::OpenOptions::new().append(true).open(&new_program)?.write_all(b"successor")?;
    let new_hash = hash(&new_program)?;

    let gate = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let address = gate.local_addr()?;
    let delayed = root.join("delayed.sh");
    let output = root.join("descendant.log");
    let status = root.join("descendant.status");
    fs::write(
        &delayed,
        format!(
            "fastctx wait '{address}'\nfastctx --version || exit\nprintf 'bundled-ripgrep\\n' | rg '^bundled-ripgrep$'\nresult=$?\nprintf '%s\\n' \"$result\" > '{}'\nexit \"$result\"\n",
            display(&status)
        ),
    )?;
    let script = format!(
        "test \"$FASTCTX_TEST_PROFILE\" = yes || exit 9\ntest -z \"${{BASH_ENV+x}}\" || exit 10\n'{}' --noprofile --norc '{}' > '{}' 2>&1 < /dev/null &\nexit 0\n",
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
            answer_receipts: false,
            on: None,
            account: None,
            world: &world,
            names: &Names { me: machine.clone(), was: Vec::new(), renamed: Vec::new() },
            owner: &owner.clone().into(),
            town: None,
            request: &RunRequest {
                turn: None,
                call_id: None,
                script,
                cwd: Some(display(&root)),
                writes_off: false,
                wait: None,
                attachments: Vec::new(),
            },
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
    anyhow::ensure!(old_program == PathBuf::from(next["copy"].as_str().expect("stable copy")));
    anyhow::ensure!(hash(&old_program)? == new_hash);
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
    anyhow::ensure!(text.replace('\r', "") == format!("program {new_hash}\nbundled-ripgrep\n"), "{text}");
    anyhow::ensure!(fs::read_to_string(status)?.trim() == "0");
    let cleaned = pin(&new_program, &world, &root, &bash)?;
    anyhow::ensure!(old_program.is_file());
    println!(
        "{}",
        serde_json::json!({
            "world": world.path(), "running": running, "ended": ended,
        "during_use": next, "after_use": cleaned, "descendant_output": text, "descendant_exit": 0
        })
    );
    anyhow::ensure!(root.canonicalize()?.parent() == Some(base.canonicalize()?.as_path()));
    // Windows lets go of a program a moment after it exits; the test is done
    // by then, and only its own folder waits to go.
    let mut attempts = 0;
    while let Err(error) = fs::remove_dir_all(&root) {
        attempts += 1;
        if attempts == 50 || error.kind() != std::io::ErrorKind::PermissionDenied && error.raw_os_error() != Some(32) {
            return Err(error.into());
        }
        thread::sleep(Duration::from_millis(100));
    }
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
