use fastctx_install::uninstall::Installation;
use fastctx_types::uninstall::Event;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap();
    let executable = PathBuf::from(args.next().unwrap());
    let installation = Installation::here()?;
    if mode == "restore" {
        let _gate = fastctx_platform::uninstall_gate::Gate::try_acquire(&installation.root)?.unwrap();
        fastctx_install::uninstall::delete_file(&installation.marker())?;
        fastctx_install::uninstall::delete_file(&installation.root.join("uninstall-progress.json"))?;
        fastctx_install::apply::restore_withdrawn(&installation.places(executable))?;
        return Ok(());
    }
    let everything = mode == "erase-crash";
    let request = fastctx_uninstall::preview(installation, everything)?;
    let host = request.plan.steps.iter().find(|step| step.kind == "codex").unwrap().id.clone();
    tokio::runtime::Runtime::new()?.block_on(fastctx_uninstall::run(request, |event| {
        println!("{}", serde_json::to_string(&event).unwrap());
        if matches!(&event, Event::Step { step, .. } if step == &host) {
            std::process::exit(99);
        }
    }))
}
