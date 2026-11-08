//! Output retention remains readable by both old and new clients.

use fastctx_types::data::{DataReply, Kinds, Space};

pub fn fixtures() -> Vec<DataReply> {
    let listed = |policy: Option<&str>| {
        DataReply::Listed(Box::new(Space {
            machine: "local".parse().expect("machine"),
            at: "2026-01-01T00:00:00Z".parse().expect("timestamp"),
            root: "/world".into(),
            root_bytes: 0,
            kinds: Kinds::default(),
            conversations: vec![],
            output_days: 30,
            output_retention: policy.map(str::to_owned),
            claude_logins: vec![],
        }))
    };
    vec![
        listed(None),
        listed(Some("unlimited")),
        DataReply::OutputDays { days: 30, output_retention: None },
        DataReply::OutputDays { days: 30, output_retention: Some("unlimited".into()) },
    ]
}
