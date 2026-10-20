//! Transitional assisted sessions can view only their recorded attachments.

use super::*;
use fastctx_types::run::*;

pub(crate) async fn run(
    node: &Arc<Node>,
    resident: &ResidentId,
    request: &RunRequest,
) -> Option<Result<RunReply, Failure>> {
    let arguments = fastctx_hands::run::preread::attachment_view(&request.script)?;
    let (node, resident, request) = (node.clone(), resident.clone(), request.clone());
    Some(
        async move {
            let place = node.places.using(resident.as_str()).await;
            node.residents.active(&resident)?;
            tokio::task::spawn_blocking(move || {
                let _place = place;
                read(&node, &resident, &request, &arguments)
            })
            .await
            .map_err(failure)?
        }
        .await,
    )
}

fn read(node: &Node, resident: &ResidentId, request: &RunRequest, arguments: &[String]) -> Result<RunReply, Failure> {
    let (options, requested) = options(arguments)?;
    let attached = references::listed(node, resident)?;
    let mut paths = Vec::new();
    for path in requested {
        // 2026-10-20: an exact server-written receipt, never a caller's path prefix,
        // grants this read. Another conversation and arbitrary files are excluded.
        let receipt = attached
            .iter()
            .find(|file| node.is_me(&file.owner.machine) && file.path == path)
            .ok_or_else(|| Failure::not_run("this reader only opens files attached to this conversation"))?;
        let (path, _) = verified_original(node, receipt, &Default::default())?;
        paths.push(PathBuf::from(path));
    }
    let at = jiff::Timestamp::now();
    let directory = node.world.conversation(resident);
    let (job, output_path) = fastctx_hands::run::launch::reserve(&directory).map_err(failure)?;
    let mut output = std::fs::OpenOptions::new().write(true).open(&output_path).map_err(failure)?;
    let viewed = fastctx_hands::view::view(&paths, &options, &mut output).map_err(failure)?;
    output.sync_all().map_err(failure)?;
    drop(output);
    let status = ExitStatus::Code(i64::from(viewed.failed));
    let ended = jiff::Timestamp::now();
    let record = JobRecord {
        call_id: request.call_id.clone(),
        id: job.clone(),
        machine: node.machine(),
        owner: Address { machine: node.machine(), resident: resident.clone() }.into(),
        town: node.residents.town_of(resident).flatten(),
        script: request.script.clone(),
        cwd: crate::world::native(directory.path()),
        stdin: None,
        output: crate::world::native(&output_path),
        stderr: None,
        started_at: at,
        program: None,
        supervisor: None,
        pid: None,
        exit: Some(Exited { status, at: ended }),
        output_ended_at: Some(ended),
        output_drained: None,
        remaining: Some(0),
        output_lost: None,
        stderr_lost: None,
        stop: None,
        end: Some(Ended { at: ended, how: EndedHow::Finished }),
        ends_with_node: false,
        writes_off: true,
        on: None,
        account: None,
    };
    let record_path = directory.job_record(&job);
    fastctx_hands::run::record::write(&record_path, &record).map_err(failure)?;
    let mut reply = RunReply {
        run: None,
        consumed: None,
        machine: node.machine(),
        job,
        state: RunState::Ended { status },
        output: fastctx_hands::run::window::window(&output_path).map_err(failure)?,
        visited: Vec::new(),
        answered: None,
        privacy: Vec::new(),
    };
    reply.run = Some(Box::new(crate::run_facts::assemble(&reply, &record_path).map_err(failure)?));
    if let Some(id) = &request.call_id {
        crate::run_facts::keep_reply(node, resident, id, reply.run.as_ref().expect("facts")).map_err(failure)?;
    }
    let action = crate::jobs::action(&reply, &request.script, at);
    crate::residents::record_action(&node.world, resident, &action).map_err(failure)?;
    Ok(reply)
}

fn options(arguments: &[String]) -> Result<(fastctx_hands::view::Options, Vec<String>), Failure> {
    let mut options = fastctx_hands::view::Options { in_run: true, ..Default::default() };
    let mut paths = Vec::new();
    let mut arguments = arguments.iter();
    while let Some(word) = arguments.next() {
        let value = |args: &mut std::slice::Iter<'_, String>| {
            args.next().cloned().ok_or_else(|| failure(format!("{word} needs a value")))
        };
        match word.as_str() {
            "--" => {
                paths.extend(arguments.cloned());
                break;
            }
            "--lines" => options.lines = Some(range(&value(&mut arguments)?)?),
            "--chars" => options.chars = Some(range(&value(&mut arguments)?)?),
            "--pages" => options.pages = Some(range(&value(&mut arguments)?)?),
            "--image" => options.image = true,
            "--master" => options.master = true,
            "--sheet" => options.sheet = Some(value(&mut arguments)?),
            "--sheet-index" => options.sheet_index = Some(value(&mut arguments)?.parse().map_err(failure)?),
            "--range" => options.range = Some(value(&mut arguments)?.parse().map_err(failure)?),
            "--encoding" => options.encoding = Some(value(&mut arguments)?),
            _ if word.starts_with('-') => {
                return Err(failure(format!("{word} is not supported by the attachment reader")));
            }
            _ => paths.push(word.clone()),
        }
    }
    if paths.is_empty() {
        return Err(failure("name the attached files to view"));
    }
    Ok((options, paths))
}

fn range(text: &str) -> Result<(u64, u64), Failure> {
    let (first, last) = text.split_once('-').unwrap_or((text, text));
    let (first, last): (u64, u64) = (first.parse().map_err(failure)?, last.parse().map_err(failure)?);
    if first == 0 || last < first {
        return Err(failure("ranges count from 1 and end after their start"));
    }
    Ok((first, last))
}
