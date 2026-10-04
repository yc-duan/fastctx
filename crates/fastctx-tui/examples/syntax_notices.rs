//! Prints the notices for syntax definitions embedded in the TUI.

fn main() {
    println!("## Embedded syntax definitions\n");
    println!("Syntax curation and dump generation derive from https://github.com/sharkdp/bat.\n");
    println!("Copyright (c) 2018-2021 bat-developers (https://github.com/sharkdp/bat).\n");
    println!(
        "bat is made available under the terms of either the MIT License or the Apache License 2.0, at your option.\n"
    );
    let mut notices = String::new();
    for license in two_face::acknowledgement::listing().for_syntaxes() {
        license.write_md(&mut notices);
    }
    print!("{notices}");
}
