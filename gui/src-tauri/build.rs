fn main() {
    let attributes = tauri_build::Attributes::new()
        .plugin(
            "android-world",
            tauri_build::InlinedPlugin::new()
                .commands(&[
                    "status",
                    "configure",
                    "pending_activations",
                    "acknowledge",
                    "notify",
                    "dismiss",
                    "request_notifications",
                    "settings",
                    "scan",
                    "viewing",
                ])
                .default_permission(tauri_build::DefaultPermissionRule::AllowAllCommands),
        )
        .plugin(
            "desktop-presence",
            tauri_build::InlinedPlugin::new()
                .commands(&["notify", "dismiss", "pending", "acknowledge", "window_presence"])
                .default_permission(tauri_build::DefaultPermissionRule::AllowAllCommands),
        )
        .plugin(
            "world-titlebar",
            tauri_build::InlinedPlugin::new()
                .commands(&["update", "action", "detach"])
                .default_permission(tauri_build::DefaultPermissionRule::AllowAllCommands),
        );
    tauri_build::try_build(attributes).expect("building World's shell");
}
