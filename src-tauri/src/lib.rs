mod analytics;
mod auth;
mod cloudflare;
mod commands;
mod db;
mod entities;
mod imaging;
mod media_keys;
mod update;

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

            // In development, seed an empty database with sample posts.
            #[cfg(debug_assertions)]
            if let Err(e) = tauri::async_runtime::block_on(db::seed_sample_posts(&conn)) {
                log::warn!("sample post seed skipped: {e}");
            }

            app.manage(conn);

            // Wire up the OS keychain before reading credentials so the API
            // token can be loaded from (and saved to) secure storage.
            auth::init_keystore();

            // Load the stored Cloudflare credentials (falling back to env vars)
            // into the process global used by the cloud commands.
            let initial_creds =
                auth::load_from_disk(&handle).or_else(|| cloudflare::CloudflareConfig::from_env().ok());
            auth::set_creds(initial_creds);

            Ok(())
        })
        .manage(update::PendingUpdate::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            // Auth / session
            auth::save_credentials,
            auth::clear_credentials,
            auth::get_credentials,
            auth::session_status,
            auth::save_settings,
            // Analytics (Cloudflare GraphQL)
            analytics::fetch_analytics,
            commands::upload_article,
            commands::stage_image,
            // Posts — local SQLite
            commands::create_post,
            commands::list_posts,
            commands::get_post,
            commands::update_post,
            commands::delete_post,
            // Posts — Cloudflare D1
            commands::d1_create_post,
            commands::d1_list_posts,
            commands::d1_get_post,
            commands::d1_update_post,
            commands::d1_delete_post,
            // Series — local SQLite
            commands::create_series,
            commands::list_series,
            commands::get_series,
            commands::update_series,
            commands::delete_series,
            // Series — Cloudflare D1
            commands::d1_create_series,
            commands::d1_list_series,
            commands::d1_get_series,
            commands::d1_update_series,
            commands::d1_delete_series,
            // Publish staging (local table + D1 sync)
            commands::set_post_stage,
            commands::get_post_stage,
            commands::list_posts_by_stage,
            commands::publish_post,
            commands::unpublish_post,
            // Sync
            commands::sync_posts,
            commands::sync_posts_from_cloud,
            // Post content
            commands::read_post_markdown,
            commands::save_post,
            // Media library
            commands::upload_media,
            commands::list_media,
            commands::delete_media,
            commands::stage_media_from_library,
            // Post thumbnail
            commands::set_post_thumbnail,
            // Self-update (GitHub Releases)
            update::check_for_update,
            update::install_update,
            update::restart_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
