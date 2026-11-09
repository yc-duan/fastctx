//! The tools a sentinel's model is given, and how what it calls them with
//! becomes a look or a reflex (`world-senses.md`, 哨兵是世界自己的 agent).
//! A look and a reflex's probe are written alike: the probe names the look
//! it repeats and gives its arguments, as the model would call it.
use crate::{ports::Machine, words};
use fastctx_agent::model::Tool;
use fastctx_types::{
    MachineName,
    inspect::{Inspect, MOST_LINES},
    sentinel::{Kind, Probe},
};
use serde_json::{Value, json};

/// The looks at a machine, by the names their tools are called.
pub(crate) const LOOKS: &[&str] = &["read_file", "list_directory", "processes", "resources", "jobs", "job_output"];

/// The tools of a sentinel of `kind`, its looks naming `machines`.
pub(crate) fn tools(kind: Kind, machines: &[Machine]) -> Vec<Tool> {
    let machine = machine(machines);
    let mut tools = match kind {
        Kind::Local | Kind::Goal => looks(&machine),
        Kind::Web => vec![
            tool("fetch_web", words::FETCH, json!({"url":string()}), &["url"]),
            tool("search", words::SEARCH, json!({"query":string()}), &["query"]),
        ],
    };
    if kind == Kind::Goal {
        tools.push(tool(
            "report",
            words::GOAL_REPORT,
            json!({"sound":{"type":"boolean"},"reason":string(),"looked":string(),"saw":string(),"thinks":string()}),
            &["sound"],
        ));
        return tools;
    }
    let probe = if kind == Kind::Web {
        json!({"type":["object","null"],"description":words::PROBE,
            "properties":{"tool":{"enum":["fetch_web"]},"url":string()},"required":["tool","url"]})
    } else {
        json!({"type":["object","null"],"description":words::PROBE,
            "properties":{"tool":{"enum":LOOKS},"machine":machine,"path":string(),"tail":tail(),
                "matching":matching(),"pattern":string(),"job":string()},
            "required":["tool","machine"]})
    };
    tools.push(tool(
        "install_reflex",
        words::REFLEX,
        json!({
            "id":string(),
            "at":{"type":["integer","null"],"description":words::REFLEX_AT},
            "every_ms":{"type":["integer","null"],"description":words::REFLEX_EVERY},
            "probe":probe,
            "predicate":{"type":["object","null"],"description":words::PREDICATE,
                "properties":{"kind":{"enum":["contains","absent","number","changed"]},"text":string(),"prefix":string(),
                    "comparison":{"enum":["less","less_equal","greater","greater_equal","equal"]},"value":{"type":"number"}},
                "required":["kind"]}
        }),
        &["id"],
    ));
    tools.push(tool(
        "report",
        words::REPORT,
        json!({
            "status":{"enum":["watching","satisfied","unable"]},
            "reason":{"type":"string","description":words::REPORT_REASON},
            "obstacle":{"type":"string","description":words::REPORT_OBSTACLE},
            "evidence":{"type":"array","items":string(),"description":words::REPORT_EVIDENCE},
            "plan":string(),"notes":string()
        }),
        &["status", "reason"],
    ));
    tools
}

/// The six looks at a machine.
fn looks(machine: &Value) -> Vec<Tool> {
    vec![
        tool(
            "read_file",
            words::READ_FILE,
            json!({"machine":machine,"path":string(),"tail":tail(),"matching":matching()}),
            &["machine", "path"],
        ),
        tool(
            "list_directory",
            words::LIST_DIRECTORY,
            json!({"machine":machine,"path":string(),"pattern":{"type":"string","description":words::PATTERN}}),
            &["machine", "path"],
        ),
        tool("processes", words::PROCESSES, json!({"machine":machine,"matching":matching()}), &["machine"]),
        tool("resources", words::RESOURCES, json!({"machine":machine}), &["machine"]),
        tool("jobs", words::JOBS, json!({"machine":machine}), &["machine"]),
        tool(
            "job_output",
            words::JOB_OUTPUT,
            json!({"machine":machine,"job":string(),"tail":tail(),"matching":matching()}),
            &["machine", "job"],
        ),
    ]
}

/// The machine and the look a look tool is called with.
pub(crate) fn look_of(tool: &str, arguments: &Value) -> Result<(MachineName, Inspect), String> {
    let machine = required(arguments, "machine")?.parse().map_err(|e: fastctx_types::NameError| e.to_string())?;
    let mut fields = arguments.as_object().cloned().unwrap_or_default();
    fields.remove("machine");
    fields.remove("tool");
    fields.retain(|_, value| !value.is_null());
    fields.insert("kind".into(), tool.into());
    let inspect: Inspect = serde_json::from_value(Value::Object(fields)).map_err(|e| format!("{tool}: {e}"))?;
    inspect.check()?;
    Ok((machine, inspect))
}

/// The probe a reflex of a sentinel of `kind` is installed with.
pub(crate) fn probe_of(kind: Kind, probe: &Value) -> Result<Probe, String> {
    let tool = probe["tool"].as_str().ok_or("a probe names the tool it repeats")?;
    match kind {
        Kind::Web if tool == "fetch_web" => Ok(Probe::Web { url: required(probe, "url")?.to_string() }),
        Kind::Local if LOOKS.contains(&tool) => {
            look_of(tool, probe).map(|(machine, inspect)| Probe::Look { machine, inspect })
        }
        _ => Err(format!("this sentinel's reflexes cannot repeat {tool}")),
    }
}

/// A probe as the model wrote it: the tool it repeats and its arguments.
pub(crate) fn probe_shown(probe: &Probe) -> Value {
    match probe {
        Probe::Web { url } => json!({"tool":"fetch_web","url":url}),
        Probe::Look { machine, inspect } => {
            let mut shown = serde_json::to_value(inspect).unwrap_or_default();
            if let Some(fields) = shown.as_object_mut() {
                let tool = fields.remove("kind").unwrap_or_default();
                fields.insert("tool".into(), tool);
                fields.insert("machine".into(), json!(machine));
            }
            shown
        }
    }
}

/// What a look was, as its observation names it: the machine, the tool and
/// its arguments.
pub(crate) fn source(machine: &MachineName, inspect: &Inspect) -> String {
    let mut shown = serde_json::to_value(inspect).unwrap_or_default();
    let tool = shown.as_object_mut().and_then(|fields| fields.remove("kind")).unwrap_or_default();
    format!("{machine}: {} {shown}", tool.as_str().unwrap_or_default())
}

pub(crate) fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 8192)
        .ok_or_else(|| format!("missing or oversized {key}"))
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Tool {
    Tool {
        name: name.into(),
        description: description.into(),
        parameters: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
    }
}
fn string() -> Value {
    json!({"type":"string"})
}
fn tail() -> Value {
    json!({"type":"integer","minimum":1,"maximum":MOST_LINES,"description":words::TAIL})
}
fn matching() -> Value {
    json!({"type":"string","description":words::MATCHING})
}
/// A machine parameter that names one of the machines it may look at.
fn machine(machines: &[Machine]) -> Value {
    if machines.is_empty() {
        return string();
    }
    let names: Vec<_> = machines.iter().map(|machine| machine.name.as_str()).collect();
    json!({"type":"string","enum":names})
}
