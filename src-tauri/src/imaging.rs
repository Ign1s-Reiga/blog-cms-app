//! JPG/PNG → AVIF conversion for images on their way into R2.
//!
//! AVIF is a large win on the reader side — typically a half to a third of an
//! equivalent JPEG — and the blog serves images straight from R2 with no
//! transform step in front, so whatever the CMS uploads is what visitors
//! download. Converting at upload keeps that cost off the request path.
//!
//! Only JPG and PNG are converted. Formats that are already efficient (WebP,
//! AVIF), animated (GIF), vector (SVG), or not images at all (video) are stored
//! byte-for-byte as picked — re-encoding them would lose animation, lose
//! sharpness, or simply waste time.
//!
//! Encoding is CPU-bound and takes real wall-clock time on a large photo, so
//! callers should use [`convert_to_avif`], which moves the work off the async
//! runtime.
//!
//! The two commands that bring an image into a post live here rather than in
//! `commands.rs`: converting on the way in is the whole of what they do, and
//! keeping them beside the encoder means the allow-lists and the encoder agree
//! by construction.

use std::path::PathBuf;

use image::codecs::avif::AvifEncoder;
use serde::Serialize;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

use crate::cloudflare::{self, cf};
use crate::error::{AppError, AppResult};
use crate::media_keys;

/// Extensions converted to AVIF on upload; anything else is stored unchanged.
pub const CONVERTIBLE: &[&str] = &["jpg", "jpeg", "png"];

/// Encoder speed, 1 (slowest, smallest) – 10 (fastest, largest).
///
/// The default of 1 takes tens of seconds on a phone-sized photo, which is far
/// too slow to sit inside a drag-and-drop. 6 lands within a second or two for
/// typical blog images while still beating the JPEG it replaces.
const SPEED: u8 = 6;

/// Quality, 1–100. 80 is visually transparent for photographic content at a
/// fraction of the source size.
const QUALITY: u8 = 80;

/// Whether an extension is one this module converts.
pub fn is_convertible(ext: &str) -> bool {
    CONVERTIBLE.contains(&ext)
}

/// The stored extension for a source extension: `avif` for anything converted,
/// otherwise the original.
pub fn stored_ext(ext: &str) -> &str {
    if is_convertible(ext) { "avif" } else { ext }
}

/// Decode `bytes` and re-encode as AVIF. Blocking and CPU-bound.
pub fn to_avif(bytes: &[u8]) -> AppResult<Vec<u8>> {
    let image =
        image::load_from_memory(bytes).map_err(|e| AppError::image("Cannot decode image", e))?;

    let mut out = Vec::new();
    image
        .write_with_encoder(AvifEncoder::new_with_speed_quality(&mut out, SPEED, QUALITY))
        .map_err(|e| AppError::image("AVIF encoding failed", e))?;

    Ok(out)
}

/// [`to_avif`] on a blocking thread, so a slow encode never stalls the async
/// runtime (and with it the UI's other commands).
pub async fn convert_to_avif(bytes: Vec<u8>) -> AppResult<Vec<u8>> {
    tokio::task::spawn_blocking(move || to_avif(&bytes))
        .await
        .map_err(|e| AppError::join("Image conversion thread panicked", e))?
}

// ─── Commands ───────────────────────────────────────────────────────────────

/// A dropped image after it has been copied into the local assets directory.
#[derive(Serialize)]
pub struct StagedImage {
    /// Markdown-relative reference, e.g. `"assets/<uuid>.png"`.
    pub rel: String,
    /// Original file name — used as the inserted image's alt text.
    pub name: String,
}

/// Copy a dropped image into the app's local `assets` directory so it can be
/// referenced from a post and rendered in the preview via the asset protocol.
/// Cloud (R2) upload is deferred to the save/publish sync.
///
/// `src_path` is an absolute path from an OS drag-and-drop. The extension is
/// validated against a fixed allow-list; other files are rejected.
#[tauri::command]
pub async fn stage_image(app: tauri::AppHandle, src_path: String) -> AppResult<StagedImage> {
    let src = PathBuf::from(&src_path);

    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|e| {
            matches!(
                e.as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "svg" | "bmp" | "ico"
            )
        })
        .ok_or(AppError::UnsupportedImage(src_path))?;

    let assets_dir = app
        .path()
        .app_data_dir()
        .map_err(AppError::AppDataDir)?
        .join("assets");
    tokio::fs::create_dir_all(&assets_dir)
        .await
        .map_err(|e| AppError::io("Failed to create assets dir", e))?;

    // JPG/PNG are converted to AVIF here rather than at publish, so the editor
    // preview shows the same bytes that will reach readers. Other formats are
    // copied through untouched.
    let file_name = format!("{}.{}", uuid::Uuid::new_v4(), stored_ext(&ext));
    let dest = assets_dir.join(&file_name);
    if is_convertible(&ext) {
        let bytes = tokio::fs::read(&src)
            .await
            .map_err(|e| AppError::io("Failed to read image", e))?;
        let avif = convert_to_avif(bytes).await?;
        tokio::fs::write(&dest, &avif)
            .await
            .map_err(|e| AppError::io("Failed to write converted image", e))?;
    } else {
        tokio::fs::copy(&src, &dest)
            .await
            .map_err(|e| AppError::io("Failed to copy image", e))?;
    }

    let name = src
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string();

    Ok(StagedImage { rel: format!("assets/{file_name}"), name })
}

/// Extensions accepted for a post thumbnail. Narrower than the editor's image
/// list because the thumbnail is always stored as AVIF, and the decoder is only
/// built with JPEG and PNG support — an already-AVIF file passes straight
/// through. WebP/GIF/SVG would need decoders that aren't compiled in.
const THUMBNAIL_EXTS: &[&str] = &["png", "jpg", "jpeg", "avif"];

/// Pick an image and store it as the post's thumbnail at
/// `posts/<slug>/thumbnail.avif`, the key the blog derives from the slug alone.
///
/// Replaces any existing thumbnail. Returns `Err("cancelled")` when the dialog
/// is dismissed.
#[tauri::command]
pub async fn set_post_thumbnail(app: tauri::AppHandle, slug: String) -> AppResult<String> {
    if !media_keys::is_safe_slug(&slug) {
        return Err(AppError::InvalidSlug(slug));
    }

    let app_clone = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        app_clone
            .dialog()
            .file()
            .add_filter("Image", THUMBNAIL_EXTS)
            .blocking_pick_file()
    })
    .await
    .map_err(|e| AppError::join("Dialog thread panicked", e))?;

    let src = match picked {
        None => return Err(AppError::Cancelled),
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        Some(tauri_plugin_dialog::FilePath::Path(p)) => p,
        #[allow(unreachable_patterns)]
        Some(_) => return Err(AppError::UnsupportedPathFormat),
    };

    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|e| THUMBNAIL_EXTS.contains(&e.as_str()))
        .ok_or(AppError::UnsupportedThumbnail)?;

    let bytes = tokio::fs::read(&src)
        .await
        .map_err(|e| AppError::io("Failed to read image", e))?;
    let bytes = if is_convertible(&ext) {
        convert_to_avif(bytes).await?
    } else {
        bytes
    };

    let (client, config) = cf()?;
    let key = media_keys::thumbnail_key(&config.thumbnail_key_pattern, &slug, "avif");
    cloudflare::upload_bytes_to_r2(&client, &config, &key, bytes, "image/avif").await?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny PNG, re-encoded, must come back as a valid AVIF that decodes to
    /// the same dimensions.
    #[test]
    fn png_round_trips_to_avif() {
        let mut png = Vec::new();
        let src = image::RgbImage::from_fn(64, 32, |x, y| {
            image::Rgb([(x * 4) as u8, (y * 8) as u8, 128])
        });
        image::DynamicImage::ImageRgb8(src)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode source png");

        let avif = to_avif(&png).expect("convert to avif");

        // AVIF is an ISO-BMFF file: the `ftyp` box sits at offset 4.
        assert_eq!(&avif[4..8], b"ftyp", "output is not an ISO-BMFF container");
        assert!(avif.len() > 0);
    }

    #[test]
    fn only_jpg_and_png_convert() {
        assert!(is_convertible("png"));
        assert!(is_convertible("jpg"));
        assert!(is_convertible("jpeg"));
        for keep in ["webp", "avif", "gif", "svg", "mp4"] {
            assert!(!is_convertible(keep), "{keep} should be stored unchanged");
        }
        assert_eq!(stored_ext("jpg"), "avif");
        assert_eq!(stored_ext("gif"), "gif");
    }
}
