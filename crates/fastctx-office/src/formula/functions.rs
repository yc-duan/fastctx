use std::collections::HashMap;
use std::sync::OnceLock;

pub(super) struct Function {
    pub name: &'static str,
    pub arguments: Option<usize>,
}

pub(super) fn get(index: u16, command: bool) -> Option<&'static Function> {
    static FUNCTIONS: OnceLock<HashMap<u16, Function>> = OnceLock::new();
    static COMMANDS: OnceLock<HashMap<u16, Function>> = OnceLock::new();
    let (cache, text) =
        if command { (&COMMANDS, include_str!("commands.txt")) } else { (&FUNCTIONS, include_str!("functions.txt")) };
    cache
        .get_or_init(|| {
            let mut result = HashMap::new();
            for line in text.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
                let fields: Vec<_> = line.split('\t').collect();
                let index = fields[0].parse().expect("frozen function table index");
                let arguments = (fields[2] == fields[3]).then(|| fields[2].parse().expect("frozen function arity"));
                result.insert(index, Function { name: fields[1], arguments });
            }
            result
        })
        .get(&index)
}
