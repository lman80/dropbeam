//! Image byte signatures keep HEIC/PNG data from being mislabeled as JPEG.
pub fn image_extension(bytes: &[u8], identifier: &str) -> &'static str {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) { return "jpg"; }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") { return "png"; }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") { return "gif"; }
    if bytes.get(0..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") { return "webp"; }
    if bytes.get(4..8) == Some(b"ftyp") {
        return match bytes.get(8..12) {
            Some(b"heic" | b"heix" | b"hevc" | b"hevx") => "heic",
            Some(b"avif" | b"avis") => "avif",
            _ => "heif",
        };
    }
    if identifier == "public.jpeg" { "jpg" } else { "img" }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_current_heic_and_generic_image_bytes() {
        assert_eq!(image_extension(b"\0\0\0\x18ftypheic", "public.image"), "heic");
        assert_eq!(image_extension(b"\0\0\0\x18ftypmif1", "public.image"), "heif");
        assert_eq!(image_extension(b"\0\0\0\x18ftypavif", "public.image"), "avif");
        assert_eq!(image_extension(b"\x89PNG\r\n\x1a\nrest", "public.jpeg"), "png");
        assert_eq!(image_extension(b"\xff\xd8\xffrest", "public.image"), "jpg");
    }
    #[test]
    fn handles_short_unknown_and_other_image_types() {
        for bytes in [b"".as_slice(), b"f", b"\0\0\0\x18ft"] { assert_eq!(image_extension(bytes, "public.image"), "img"); }
        assert_eq!(image_extension(b"GIF89a", "public.image"), "gif");
        assert_eq!(image_extension(b"RIFFxxxxWEBP", "public.image"), "webp");
        assert_eq!(image_extension(b"unknown", "public.jpeg"), "jpg");
    }
}
