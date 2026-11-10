use fastctx_net::link::{BoxFuture, Caller};
use fastctx_types::{
    access::{Purpose, TownId},
    remote::{Letter, RemoteCall, RemoteResult},
};
pub struct Node;
impl fastctx_net::link::Node for Node {
    fn answering(&self, _: &RemoteCall) -> Result<Option<Box<dyn Send>>, fastctx_types::ipc::Failure> {
        Ok(None)
    }
    fn call(&self, _: Caller, _: RemoteCall, _: Option<fastctx_net::data::Stream>) -> BoxFuture<RemoteResult> {
        Box::pin(async { Err(fastctx_types::ipc::Failure::refused("the isolated discovery node offers no execution")) })
    }
    fn letter(
        &self,
        _: Caller,
        _: Purpose,
        _: fastctx_net::message::LetterId,
        _: Letter,
    ) -> BoxFuture<std::io::Result<Result<(), String>>> {
        Box::pin(async { Ok(Ok(())) })
    }
    fn ledger_changed(&self, _: Option<TownId>) {}
    fn forget(&self, _: fastctx_net::wire::Forgetting) -> BoxFuture<fastctx_net::wire::Forgot> {
        Box::pin(async { fastctx_net::wire::Forgot::Done })
    }
    fn log(&self, line: String) {
        eprintln!("node: {line}");
    }
}
