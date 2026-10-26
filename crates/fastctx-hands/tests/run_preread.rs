//! The pre-execution boundary is a contract: rejected source cannot reserve
//! a job, start its supervisor, or execute an otherwise valid first command.

use fastctx_hands::run::{self, preread};
use fastctx_types::layout::{Names, WorldDir};
use fastctx_types::run::RunRequest;
use fastctx_types::{Address, MachineName, ResidentId};

#[test]
fn rejection_precedes_all_job_side_effects() {
    let directory = tempfile::tempdir().unwrap();
    let world = WorldDir::under(directory.path());
    let marker = directory.path().join("executed");
    let path = marker.to_str().unwrap().replace('\\', "/").replace('\'', "'\\''");
    let machine: MachineName = "preread-test".parse().unwrap();
    let names = Names { me: machine.clone(), was: Vec::new(), renamed: Vec::new() };
    let owner =
        Address { machine: machine.clone(), resident: ResidentId::try_from("lab-cafe".to_string()).unwrap() }.into();
    for tail in [
        "echo 'unclosed\n",
        "cat <<'END'\ncat <<'END'\nbody\nEND\n:\nEND\n",
        "on target fastctx write /file <<END\n$HOME\nEND\n",
        "cat <<'END'\nmissing delimiter\n",
    ] {
        let request = RunRequest {
            turn: None,
            call_id: None,
            script: format!("printf executed > '{path}'\n{tail}"),
            cwd: None,
            writes_off: false,
            wait: None,
        };
        let result = run::launch(&run::Launch {
            answer_receipts: false,
            on: None,
            account: None,
            world: &world,
            names: &names,
            owner: &owner,
            town: None,
            request: &request,
            home: directory.path(),
            executable: &directory.path().join("supervisor-must-not-start"),
            environment: &[],
        });
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("rejected source started a job"),
        };
        assert!(error.message.contains("line"), "{error:?}");
        assert!(!marker.exists());
        assert!(!world.path().exists(), "rejection reserved a job");
    }
}

#[test]
fn bash_and_scanner_preserve_common_source_forms() {
    // Extglob is enabled while checking because -n does not execute shopt.
    // Quoted documents and repeated delimiters in data remain literal.
    let source = r#"shopt -s extglob
printf '%s' @(one|two)
((bits = 1 << 4))
for file in a b; do
    write "$file" <<'END'
$HOME `command` \\ literal text
END
done
cat <<END <<'SECOND'
plain text
END
END
SECOND
printf '%s' END
"#;
    preread::check(source).unwrap();
}

#[test]
fn source_depth_does_not_exhaust_the_callers_stack() {
    // This is the observed Windows stack-overflow reproducer; source nesting
    // consumes growable storage rather than imposing a language depth limit.
    let source = format!("{}:{}\ncat <<'END'\nliteral\nEND\n", "echo $(".repeat(256), ")".repeat(256));
    preread::check(&source).unwrap();
}
