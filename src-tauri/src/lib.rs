mod cloudflare;
mod commands;
mod db;
mod entities;

use sea_orm::DatabaseConnection;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Open the local SQLite cache and expose the connection to commands
            // through managed state. Blocking here keeps the DB ready before the
            // first command can run.
            let handle = app.handle().clone();
            let conn: DatabaseConnection = tauri::async_runtime::block_on(db::connect(&handle))
                .expect("failed to initialise local database");
            app.manage(conn);

            Ok(())
        })
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::upload_article,
            commands::stage_image,
            commands::create_post,
            commands::list_posts,
            commands::get_post,
            commands::update_post,
            commands::delete_post,
            commands::d1_create_post,
            commands::d1_list_posts,
            commands::d1_get_post,
            commands::d1_update_post,
            commands::d1_delete_post,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
