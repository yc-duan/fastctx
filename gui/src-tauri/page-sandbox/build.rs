fn main() {
    tauri_plugin::Builder::new(&["mount", "place", "post", "close", "font"]).android_path("android").build();
}
