// A window, not a console, on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    fastctx_gui_lib::run();
}
