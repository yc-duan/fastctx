//! Process-level lifecycle contract, using a private World and a local node
//! protocol peer. The gates establish ordering without runtime deadlines.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use fastctx_hands::run::{self, record, shell};
use fastctx_platform::ipc::{Endpoint, Listener};
use fastctx_platform::{current_identity, ensure_private_dir, is_alive};
use fastctx_types::ipc::{ClientFrame, Hello, NodeFrame, Peer, Welcome};
use fastctx_types::layout::{Names, WorldDir};
use fastctx_types::run::{EndedHow, ExitStatus, JobRecord, OutputMode, RunRequest, StopRequest};
use fastctx_types::{Address, Call, MachineName, ProcessIdentity};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() -> Result {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match args.first().and_then(|arg| arg.to_str()) {
        Some("supervise") => {
            run::supervisor::supervise_with(Path::new(&args[1]), Path::new(&args[2]), |world, job| {
                let bash = PathBuf::from(std::env::var_os("FASTCTX_TEST_BASH").expect("test bash"));
                let script = world.job_owner(&job.machine, &job.owner).job_script(&job.id);
                Ok(shell::Prepared {
                    program: bash,
                    args: vec!["--noprofile".into(), "--norc".into(), display(&script).into()],
                    environment: std::env::vars_os().collect(),
                })
            })?
        }
        Some("child") => {
            let mut gate = TcpStream::connect(args[1].to_str().expect("gate address"))?;
            let identity = serde_json::to_vec(&current_identity()?)?;
            gate.write_all(&(identity.len() as u32).to_le_bytes())?;
            gate.write_all(&identity)?;
            let mut released = [0];
            gate.read_exact(&mut released)?;
            assert_eq!(released, [1]);
            // Both streams exceed pipe capacity; serial draining deadlocks.
            let error = thread::spawn(|| io::stderr().write_all(&stderr_bytes()));
            io::copy(&mut io::stdin(), &mut io::stdout())?;
            error.join().expect("stderr writer")?;
        }
        _ => exercise()?,
    }
    Ok(())
}

fn exercise() -> Result {
    let root = tempfile::Builder::new().prefix("fastctx-job-").tempdir()?;
    let root = root.path().canonicalize()?;
    let world = WorldDir::under(&root);
    ensure_private_dir(world.path())?;
    let bash = std::env::var_os("FASTCTX_TEST_BASH").map(PathBuf::from).map(Ok).unwrap_or_else(shell::bash)?;
    let executable = std::env::current_exe()?;
    let machine: MachineName = "job-test".parse()?;
    let owner = Address { machine: machine.clone(), resident: "lab-cafe".parse()? }.into();
    let environment = environment(&root, &bash);
    let request = RunRequest {
        turn: None,
        call_id: None,
        script: String::new(),
        cwd: Some(display(&root)),
        writes_off: false,
        wait: None,
    };
    let names = Names { me: machine.clone(), was: Vec::new(), renamed: Vec::new() };
    let launch = run::Launch {
        answer_receipts: false,
        world: &world,
        names: &names,
        owner: &owner,
        town: None,
        request: &request,
        home: &root,
        executable: &executable,
        environment: &environment,
        on: None,
        account: None,
    };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let notified = Arc::new(Mutex::new(Vec::<JobRecord>::new()));
    let listener = {
        let _entered = runtime.enter();
        Listener::bind(&Endpoint::for_world(world.path())?)?
    };
    let node = runtime.spawn(node(listener, world.clone(), machine.clone(), notified.clone()));
    runtime.block_on(async {
        let input = root.join("input.bin");
        let bytes: Vec<u8> = (0..=255).cycle().take(1024 * 1024).collect();
        fs::write(&input, &bytes)?;
        let (gate, address) = new_gate()?;
        let script =
            format!("exec 3<&0\n{} child {address} <&3 &\ncat {}\nexit 17\n", quote(&executable), quote(&input));
        let request =
            RunRequest { turn: None, call_id: None, script, cwd: Some(display(&root)), writes_off: false, wait: None };
        let launch = run::Launch { request: &request, ..launch };
        let job = run::launch_with_io(&launch, OutputMode::Separate, Some(&input))?;
        let (mut child, identity) = arrived(&gate, &job).await?;
        let running = until(&job, |r| r.output_drained.is_some() && r.remaining.is_some_and(|n| n > 0)).await?;
        assert!(running.end.is_none() && running.output_ended_at.is_none());
        assert_eq!(running.output_drained.unwrap().stdout, bytes.len() as u64);
        assert_eq!(fs::read(&running.output)?, bytes);
        assert_eq!(running.stdin.as_deref().map(Path::new), Some(input.as_path()));
        child.write_all(&[1])?;
        let ended = until(&job, |r| r.end.is_some()).await?;
        assert!(matches!(ended.end.as_ref().unwrap().how, EndedHow::Finished));
        assert_eq!(ended.exit.unwrap().status, ExitStatus::Code(17));
        assert_eq!(fs::read(&ended.output)?, [bytes.as_slice(), bytes.as_slice()].concat());
        assert_eq!(fs::read(ended.stderr.as_ref().unwrap())?, stderr_bytes());
        assert!(ended.output_lost.is_none() && ended.stderr_lost.is_none());
        assert!(!is_alive(&identity)?);
        wait_supervisor(&job).await?;
        assert_notified(&notified, &ended);
        println!("separate stdout/stderr, binary stdin and post-main output: passed");

        let request = RunRequest {
            turn: None,
            call_id: None,
            script: "printf out; printf err >&2; cat\n".into(),
            cwd: Some(display(&root)),
            writes_off: false,
            wait: None,
        };
        let launch = run::Launch { request: &request, ..launch };
        let job = run::launch(&launch)?;
        let ended = until(&job, |r| r.end.is_some()).await?;
        assert_eq!(fs::read(&ended.output)?, b"outerr");
        assert!(ended.stderr.is_none() && ended.stdin.is_none());
        wait_supervisor(&job).await?;
        println!("default merged order and null stdin: passed");

        // The shell's exec into a native program crosses MSYS's PID model.
        // With the main shell already gone, stopping must use tree ownership.
        for separate in [false, true] {
            let (gate, address) = new_gate()?;
            let script = format!("(exec {} child {address}) &\nexit 23\n", quote(&executable));
            let request = RunRequest {
                turn: None,
                call_id: None,
                script,
                cwd: Some(display(&root)),
                writes_off: false,
                wait: None,
            };
            let launch = run::Launch { request: &request, ..launch };
            let mode = if separate { OutputMode::Separate } else { OutputMode::Merged };
            let job = run::launch_with_io(&launch, mode, None)?;
            let (_child, identity) = arrived(&gate, &job).await?;
            let running = until(&job, |r| r.exit.is_some()).await?;
            assert_eq!(running.exit.unwrap().status, ExitStatus::Code(23));
            let request =
                StopRequest { by: "lab-cafe@job-test".into(), reason: "the pipeline downstream closed".into() };
            let (first, second) = tokio::join!(
                run::stop(&world, &job.record, job.supervisor.as_ref(), request.clone()),
                run::stop(&world, &job.record, job.supervisor.as_ref(), request.clone()),
            );
            let ended = first?;
            assert_eq!(second?, ended);
            assert!(matches!(ended.end.as_ref().unwrap().how, EndedHow::Stopped));
            assert_eq!(ended.stop.as_ref().unwrap().request, request);
            assert!(ended.output_ended_at.is_some());
            assert_eq!(ended.remaining, Some(0));
            assert!(!is_alive(&identity)?);
            wait_supervisor(&job).await?;
            assert_notified(&notified, &ended);
            let again = run::stop(
                &world,
                &job.record,
                None,
                StopRequest { by: "second caller".into(), reason: "already stopped".into() },
            )
            .await?;
            assert_eq!(again, ended);
            println!("stop after main exit, separate={separate}, ends_with_node={}: passed", ended.ends_with_node);
        }

        let (gate, address) = new_gate()?;
        let request = RunRequest {
            turn: None,
            call_id: None,
            script: format!("exec {} child {address}\n", quote(&executable)),
            cwd: Some(display(&root)),
            writes_off: false,
            wait: None,
        };
        let launch = run::Launch { request: &request, ..launch };
        let job = run::launch(&launch)?;
        let ended = run::stop(
            &world,
            &job.record,
            job.supervisor.as_ref(),
            StopRequest { by: "user".into(), reason: "stop during startup".into() },
        )
        .await?;
        assert!(matches!(ended.end.as_ref().unwrap().how, EndedHow::Stopped));
        wait_supervisor(&job).await?;
        drop(gate);
        println!("stop during startup: passed");

        #[cfg(unix)]
        {
            let (gate, address) = new_gate()?;
            let request = RunRequest {
                turn: None,
                call_id: None,
                script: format!("exec {} child {address}\n", quote(&executable)),
                cwd: Some(display(&root)),
                writes_off: false,
                wait: None,
            };
            let launch = run::Launch { request: &request, ..launch };
            let job = run::launch(&launch)?;
            let (_child, identity) = arrived(&gate, &job).await?;
            let supervisor = job.supervisor.as_ref().expect("supervisor identity");
            // SAFETY: this test started the still-live supervisor.
            assert_eq!(unsafe { libc::kill(supervisor.pid as i32, libc::SIGTERM) }, 0);
            let ended = until(&job, |r| r.end.is_some()).await?;
            assert!(matches!(ended.end.as_ref().unwrap().how, EndedHow::Stopped));
            assert_eq!(
                ended.stop.as_ref().unwrap().request.by,
                format!("process {} (uid {})", std::process::id(), unsafe { libc::getuid() })
            );
            assert!(!is_alive(&identity)?);
            wait_supervisor(&job).await?;
            assert_notified(&notified, &ended);
            println!("Unix signal stop and sender attribution: passed");
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;
    node.abort();
    Ok(())
}

async fn node(mut listener: Listener, world: WorldDir, machine: MachineName, notified: Arc<Mutex<Vec<JobRecord>>>) {
    loop {
        let mut connection = listener.accept().await.expect("node accept");
        let hello: Hello = serde_json::from_slice(&connection.recv().await.unwrap().unwrap()).unwrap();
        assert!(matches!(hello.peer, Peer::Supervisor { .. }));
        connection
            .send(
                &serde_json::to_vec(&Welcome::Accepted {
                    goal_runtime: false,
                    sentinels: false,
                    goal_commands: false,
                    profiles: false,
                    account_records: false,
                    conversation_plans: false,
                    scratch_locations: false,
                    machine: machine.clone(),
                    version: "test".into(),
                    node_lifetime: None,
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let ClientFrame::Call { id, call } =
            serde_json::from_slice(&connection.recv().await.unwrap().unwrap()).unwrap()
        else {
            panic!("expected call")
        };
        let Call::JobChanged { job, owner, town: _ } = call else { panic!("unexpected supervisor call") };
        let record = record::read(&world.job_owner(&machine, &owner).job_record(&job)).unwrap();
        notified.lock().unwrap().push(record);
        // The supervisor only needs the call to complete; a refused reply
        // still proves delivery through the actual node protocol.
        let reply = NodeFrame::Reply { id, reply: Err(fastctx_types::ipc::Failure::new("test observer")) };
        connection.send(&serde_json::to_vec(&reply).unwrap()).await.unwrap();
        let (_, mut writer) = connection.split();
        writer.drain().await.unwrap();
    }
}

async fn until(job: &run::Launched, condition: impl Fn(&JobRecord) -> bool) -> Result<JobRecord> {
    loop {
        let record = record::read(&job.record)?;
        if condition(&record) {
            return Ok(record);
        }
        assert!(record.end.is_none(), "job ended before its gate: {record:?}");
        if let Some(identity) = &job.supervisor {
            assert!(is_alive(identity)?, "supervisor died before its gate");
        }
        // Sample a live process's record; this never sets a deadline.
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn new_gate() -> Result<(TcpListener, std::net::SocketAddr)> {
    let gate = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    gate.set_nonblocking(true)?;
    let address = gate.local_addr()?;
    Ok((gate, address))
}

async fn arrived(gate: &TcpListener, job: &run::Launched) -> Result<(TcpStream, ProcessIdentity)> {
    loop {
        match gate.accept() {
            Ok((mut stream, _)) => {
                // Windows inherits the listener's nonblocking mode; this
                // frame is read synchronously after the peer connects.
                stream.set_nonblocking(false)?;
                let mut size = [0; 4];
                stream.read_exact(&mut size)?;
                let mut identity = vec![0; u32::from_le_bytes(size) as usize];
                stream.read_exact(&mut identity)?;
                return Ok((stream, serde_json::from_slice(&identity)?));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                assert!(record::read(&job.record)?.end.is_none(), "job ended before child arrived");
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

async fn wait_supervisor(job: &run::Launched) -> Result {
    if let Some(identity) = &job.supervisor {
        while is_alive(identity)? {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
    Ok(())
}

fn assert_notified(notified: &Mutex<Vec<JobRecord>>, ended: &JobRecord) {
    assert!(notified.lock().unwrap().iter().any(|r| r == ended), "final JobChanged was not delivered");
}

fn stderr_bytes() -> Vec<u8> {
    (0..=255).rev().cycle().take(1024 * 1024).collect()
}
fn display(path: &Path) -> String {
    path.to_string_lossy().trim_start_matches(r"\\?\").replace('\\', "/")
}
fn quote(path: &Path) -> String {
    format!("'{}'", display(path).replace('\'', "'\\''"))
}

fn environment(root: &Path, bash: &Path) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = [
        ("HOME", root.as_os_str()),
        ("USERPROFILE", root.as_os_str()),
        ("FASTCTX_TEST_BASH", bash.as_os_str()),
        ("FASTCTX_HOME", root.as_os_str()),
        ("TEMP", root.as_os_str()),
        ("TMP", root.as_os_str()),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.to_owned()))
    .collect();
    if let Some(system) = std::env::var_os("SYSTEMROOT") {
        env.push(("SYSTEMROOT".into(), system));
    }
    let tools = bash.parent().expect("bash directory");
    env.push(("PATH".into(), tools.as_os_str().to_owned()));
    env
}
