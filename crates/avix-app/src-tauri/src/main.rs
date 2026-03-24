#![cfg_attr(
    all(not(debug_assertions), target_os = \"windows\"),
    windows_subsystem = \"windows\"
)]

fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!(\"avix-app starting\");

    let app_state = avix_app::create_app_state();

    tauri::Builder::default()
        .manage(app_state)
        .run(tauri::generate_context!())
        .expect(\"error while running tauri application\");
}