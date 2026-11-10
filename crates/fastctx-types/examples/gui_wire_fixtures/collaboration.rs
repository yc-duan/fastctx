use fastctx_types::collaboration::{Branch, NativeAgent, Tree};

pub fn fixtures() -> Vec<Tree> {
    [false, true]
        .into_iter()
        .map(|reading| Tree {
            conversation: Branch {
                address: "codex-ab12@desktop".parse().unwrap(),
                label: None,
                born_at: None,
                now: None,
                goal: Ok(None),
                plan: None,
                agents: vec![NativeAgent { id: "agent-1".into(), reading, ..Default::default() }],
                reading,
                unavailable: Vec::new(),
            },
            colleagues: Vec::new(),
        })
        .collect()
}
