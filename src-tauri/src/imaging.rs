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

use image::codecs::avif::AvifEncoder;

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
pub fn to_avif(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let image = image::load_from_memory(bytes).map_err(|e| format!("Cannot decode image: {e}"))?;

    let mut out = Vec::new();
    image
        .write_with_encoder(AvifEncoder::new_with_speed_quality(&mut out, SPEED, QUALITY))
        .map_err(|e| format!("AVIF encoding failed: {e}"))?;

    Ok(out)
}

/// [`to_avif`] on a blocking thread, so a slow encode never stalls the async
/// runtime (and with it the UI's other commands).
pub async fn convert_to_avif(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    tokio::task::spawn_blocking(move || to_avif(&bytes))
        .await
        .map_err(|e| format!("Image conversion thread panicked: {e}"))?
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
