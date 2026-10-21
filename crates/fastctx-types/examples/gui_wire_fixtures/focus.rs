use fastctx_types::focus::*;
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
    let starts = Folders {
        machine: machine.clone(),
        path: None,
        parent: None,
        home: "C:/Users/ada".into(),
        folders: vec![
            Folder { name: "C:/Users/ada".into(), path: "C:/Users/ada".into(), hidden: false },
            Folder { name: "D:/".into(), path: "D:/".into(), hidden: false },
        ],
    };
    let inside = Folders {
        machine: machine.clone(),
        path: Some("C:/work".into()),
        parent: Some("C:/".into()),
        home: "C:/Users/ada".into(),
        folders: vec![
            Folder { name: ".cache".into(), path: "C:/work/.cache".into(), hidden: true },
            Folder { name: "中文 项目".into(), path: "C:/work/中文 项目".into(), hidden: false },
        ],
    };
    Fixtures {
        calls: vec![
            Call::Folders { machine: None, path: None },
            Call::Folders { machine: Some(machine.clone().into()), path: Some("C:/work".into()) },
        ],
        replies: vec![Reply::Folders(starts.clone()), Reply::Folders(inside.clone())],
        remote_calls: vec![
            RemoteCall::Folders { path: Some("/home/ada".into()) },
            RemoteCall::Outline { folders: vec!["/home/ada/app".into()], within_ms: 4000 },
        ],
        remote_results: vec![
            Ok(RemoteReply::Folders(inside)),
            Ok(RemoteReply::Outlined {
                outlines: vec![
                    Outlined { path: "/home/ada/app".into(), outline: Ok("/home/ada/app\n  folders: src (3)".into()) },
                    Outlined { path: "/srv/data".into(), outline: Err("permission denied".into()) },
                ],
            }),
            Err(Failure::new("/srv/data: permission denied")),
        ],
    }
}
