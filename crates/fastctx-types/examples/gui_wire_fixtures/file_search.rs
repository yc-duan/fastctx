use fastctx_types::file_search::*;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply, RemoteResult};
use fastctx_types::{Call, Reply};
use serde::Serialize;

#[derive(Serialize)]
pub struct Fixtures {
    calls: Vec<Call>,
    replies: Vec<Reply>,
    remote_calls: Vec<RemoteCall>,
    remote_results: Vec<RemoteResult>,
}

pub fn fixtures() -> Fixtures {
    let machine: fastctx_types::MachineName = "desktop".parse().unwrap();
    let search = "79dcc61ceab94d2dac41c71c5bd9f828".to_string();
    let requests = vec![
        FileSearchRequest::Start {
            scope: FileSearchScope::Resident { resident: "codex-a1b2".parse().unwrap(), roots: vec![] },
        },
        FileSearchRequest::Start { scope: FileSearchScope::Roots { roots: vec!["C:/work/中文 项目".into()] } },
        FileSearchRequest::Query { search: search.clone(), query: "调用.rs".into(), limit: 20 },
        FileSearchRequest::Close { search: search.clone() },
    ];
    let results = [
        FileSearchResult::Started { search },
        FileSearchResult::Matches { query: "调用.rs".into(), ready: false, total: 0, files: vec![] },
        FileSearchResult::Matches {
            query: "调用.rs".into(),
            ready: false,
            total: 1,
            files: vec![FileMatch {
                path: "C:/work/中文 项目/src/调用.rs".into(),
                root: "C:/work/中文 项目".into(),
                relative: "src/调用.rs".into(),
            }],
        },
        FileSearchResult::Matches {
            query: "调用.rs".into(),
            ready: true,
            total: 2,
            files: vec![FileMatch {
                path: "C:/work/中文 项目/src/调用.rs".into(),
                root: "C:/work/中文 项目".into(),
                relative: "src/调用.rs".into(),
            }],
        },
        FileSearchResult::Matches { query: "missing".into(), ready: true, total: 0, files: vec![] },
        FileSearchResult::Closed,
    ]
    .into_iter()
    .map(|result| FileSearchReply { machine: machine.clone(), result })
    .collect::<Vec<_>>();
    let mut remote_results =
        results.iter().cloned().map(|reply| Ok(RemoteReply::FilesFound(reply))).collect::<Vec<_>>();
    remote_results.push(Err(Failure::new("file search is closed or belongs to another caller")));
    Fixtures {
        calls: requests
            .iter()
            .flat_map(|request| {
                [
                    Call::FindFiles { machine: None, request: request.clone() },
                    Call::FindFiles { machine: Some(machine.clone().into()), request: request.clone() },
                ]
            })
            .collect(),
        remote_calls: requests.into_iter().map(|request| RemoteCall::FindFiles { request }).collect(),
        replies: results.into_iter().map(Reply::FilesFound).collect(),
        remote_results,
    }
}
