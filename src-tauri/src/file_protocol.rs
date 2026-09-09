//! Local previews. Every function here is called on a blocking worker, never the UI thread.
use std::{fs::File, io::{Read, Seek, SeekFrom}, path::{Component, Path, PathBuf}};
use tauri::http::{Request, Response};

const MAX_CHUNK: u64 = 1000 * 1024;
const MAX_RANGES: usize = 16;

pub(crate) fn empty(status: u16) -> Response<Vec<u8>> {
    Response::builder().status(status).header("Content-Length", 0)
        .body(Vec::new()).unwrap()
}

fn io_status(e: std::io::Error) -> u16 {
    if e.kind() == std::io::ErrorKind::PermissionDenied { 403 } else { 404 }
}

/// uri.path() strips cache-busting queries BEFORE decoding (%3F stays in the filename).
/// Like Tauri's asset protocol, remove exactly one leading slash and decode once.
fn decode_scoped_path(uri: &tauri::http::Uri, roots: &[PathBuf]) -> Result<PathBuf, u16> {
    let encoded = uri.path().strip_prefix('/').ok_or(403u16)?;
    let decoded = percent_encoding::percent_decode(encoded.as_bytes()).decode_utf8_lossy();
    let path = PathBuf::from(decoded.as_ref());
    if !path.is_absolute() || decoded.contains('\0')
        || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(403);
    }
    // Component-aware comparison prevents HOME-other from matching HOME.
    let path: PathBuf = path.components().collect();
    if !roots.iter().any(|root| root.is_absolute() && path.starts_with(root)) {
        return Err(403);
    }
    Ok(path)
}

fn canonical_scoped_path(path: &Path, roots: &[PathBuf]) -> Result<PathBuf, u16> {
    let canonical = path.canonicalize().map_err(io_status)?;
    // Resolve both sides (including macOS/iOS directory aliases) before opening.
    if roots.iter().filter_map(|r| r.canonicalize().ok()).any(|r| canonical.starts_with(r)) {
        Ok(canonical)
    } else { Err(403) }
}

fn ranges(value: &str, len: u64) -> Result<Vec<(u64, u64)>, ()> {
    if len == 0 || value.split(',').count() > MAX_RANGES { return Err(()); }
    let parsed = http_range::HttpRange::parse(value, len).map_err(|_| ())?;
    if parsed.is_empty() { return Err(()); }
    parsed.into_iter().map(|r| {
        if r.start >= len || r.length == 0 { return Err(()); }
        Ok((r.start, r.start + r.length.min(len - r.start).min(MAX_CHUNK) - 1))
    }).collect()
}

fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg", "png" => "image/png", "gif" => "image/gif",
        "webp" => "image/webp", "avif" => "image/avif", "svg" => "image/svg+xml",
        "bmp" => "image/bmp", "ico" => "image/x-icon", "heic" => "image/heic",
        "tif" | "tiff" => "image/tiff", "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime", "webm" => "video/webm", "ogv" => "video/ogg",
        "mp3" => "audio/mpeg", "m4a" => "audio/mp4", "wav" => "audio/wav",
        "ogg" => "audio/ogg", "pdf" => "application/pdf", _ => "application/octet-stream",
    }
}

pub(crate) fn respond(request: Request<Vec<u8>>, roots: &[PathBuf]) -> Response<Vec<u8>> {
    response(request, roots).unwrap_or_else(empty)
}

fn response(request: Request<Vec<u8>>, roots: &[PathBuf]) -> Result<Response<Vec<u8>>, u16> {
    let path = decode_scoped_path(request.uri(), roots)?;
    let path = canonical_scoped_path(&path, roots)?;
    let mut file = File::open(&path).map_err(io_status)?;
    let metadata = file.metadata().map_err(io_status)?;
    if !metadata.is_file() { return Err(403); }
    let len = metadata.len();
    let mime = mime(&path);
    let mut content_type = mime.to_string();
    let mut builder = Response::builder()
        .header("Accept-Ranges", "bytes")
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Expose-Headers", "Content-Range, Accept-Ranges");
    if request.method() == tauri::http::Method::HEAD {
        return Ok(builder.header("Content-Type", mime).header("Content-Length", len).body(Vec::new()).unwrap());
    }
    let mut body = Vec::new();
    if let Some(header) = request.headers().get("range") {
        let selected = header.to_str().map_err(|_| ()).and_then(|s| ranges(s, len));
        let selected = match selected {
            Ok(r) => r,
            Err(_) => return Ok(builder.status(416).header("Content-Range", format!("bytes */{len}"))
                .header("Content-Length", 0).body(body).unwrap()),
        };
        builder = builder.status(206);
        if selected.len() == 1 {
            let (start, end) = selected[0];
            builder = builder.header("Content-Range", format!("bytes {start}-{end}/{len}"));
            body = read_range(&mut file, start, end)?;
        } else {
            let boundary = format!("dbfile-{}", uuid::Uuid::new_v4());
            content_type = format!("multipart/byteranges; boundary={boundary}");
            for (start, end) in selected {
                body.extend_from_slice(format!("--{boundary}\r\nContent-Type: {mime}\r\nContent-Range: bytes {start}-{end}/{len}\r\n\r\n").as_bytes());
                body.extend_from_slice(&read_range(&mut file, start, end)?);
                body.extend_from_slice(b"\r\n");
            }
            body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        }
    } else {
        file.read_to_end(&mut body).map_err(io_status)?;
    }
    Ok(builder.header("Content-Type", content_type).header("Content-Length", body.len()).body(body).unwrap())
}

fn read_range(file: &mut File, start: u64, end: u64) -> Result<Vec<u8>, u16> {
    file.seek(SeekFrom::Start(start)).map_err(io_status)?;
    let mut bytes = vec![0; (end - start + 1) as usize];
    file.read_exact(&mut bytes).map_err(io_status)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(path: &str, host: &str) -> tauri::http::Uri {
        format!("{host}/{}?landed=123", percent_encoding::utf8_percent_encode(path, percent_encoding::NON_ALPHANUMERIC)).parse().unwrap()
    }

    #[test]
    fn decoding_and_scope() {
        #[cfg(not(windows))]
        let (home, config) = ("/Users/test", "/app/config");
        #[cfg(windows)]
        let (home, config) = ("C:\\Users\\test", "D:\\AppConfig");
        let roots = vec![PathBuf::from(home), PathBuf::from(config)];
        for host in ["dbfile://localhost", "http://dbfile.localhost"] {
            for root in &roots {
                let path = root.join("Desktop").join("a b+雪?#%2F.png");
                assert_eq!(decode_scoped_path(&uri(path.to_str().unwrap(), host), &roots), Ok(path));
            }
            for path in [format!("{home}/../secret"), format!("{home}/a/../../secret"),
                format!("{home}-other/image.png"), "/outside/image.png".into(), "relative.png".into()] {
                assert_eq!(decode_scoped_path(&uri(&path, host), &roots), Err(403));
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_letter() {
        let roots = vec![PathBuf::from(r"C:\Users\test")];
        let uri = "http://dbfile.localhost/C%3A%5CUsers%5Ctest%5Cphoto.png".parse().unwrap();
        assert_eq!(decode_scoped_path(&uri, &roots), Ok(PathBuf::from(r"C:\Users\test\photo.png")));
        let uri = "http://dbfile.localhost/C%3A%5CUsers%5Ctest%5C..%5Csecret".parse().unwrap();
        assert_eq!(decode_scoped_path(&uri, &roots), Err(403));
    }

    #[test]
    fn range_parsing() {
        assert_eq!(ranges("bytes=0-0", 100), Ok(vec![(0, 0)]));
        assert_eq!(ranges("bytes=10-19", 100), Ok(vec![(10, 19)]));
        assert_eq!(ranges("bytes=90-", 100), Ok(vec![(90, 99)]));
        assert_eq!(ranges("bytes=-10", 100), Ok(vec![(90, 99)]));
        assert_eq!(ranges("bytes=-200", 100), Ok(vec![(0, 99)]));
        assert_eq!(ranges("bytes=90-200", 100), Ok(vec![(90, 99)]));
        assert_eq!(ranges("bytes=0-1,90-99", 100), Ok(vec![(0, 1), (90, 99)]));
        assert_eq!(ranges("bytes=0-", MAX_CHUNK * 3), Ok(vec![(0, MAX_CHUNK - 1)]));
        for value in ["bytes=100-", "bytes=20-10", "bytes=-0", "bytes=", "garbage", "bytes=a-b", "bytes=18446744073709551616-"] {
            assert!(ranges(value, 100).is_err(), "{value}");
        }
        assert!(ranges("bytes=0-", 0).is_err());
        assert!(ranges(&format!("bytes={}", vec!["0-1"; 17].join(",")), 100).is_err());
    }

    #[test]
    fn responses_and_symlink_scope() {
        let base = std::env::temp_dir().join(format!("dbfile-test-{}", uuid::Uuid::new_v4()));
        let home = base.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let path = home.join("preview.png");
        std::fs::write(&path, b"0123456789").unwrap();
        let roots = vec![home.clone()];
        let request = |range: Option<&str>| {
            let mut builder = Request::builder().uri(uri(path.to_str().unwrap(), "dbfile://localhost"));
            if let Some(range) = range { builder = builder.header("Range", range); }
            builder.body(Vec::new()).unwrap()
        };
        let full = respond(request(None), &roots);
        assert_eq!(full.status(), 200);
        assert_eq!(full.headers()["Content-Type"], "image/png");
        assert_eq!(full.headers()["Content-Length"], "10");
        assert_eq!(full.body(), b"0123456789");
        let part = respond(request(Some("bytes=2-4")), &roots);
        assert_eq!(part.status(), 206);
        assert_eq!(part.headers()["Content-Range"], "bytes 2-4/10");
        assert_eq!(part.headers()["Accept-Ranges"], "bytes");
        assert_eq!(part.body(), b"234");
        let multi = respond(request(Some("bytes=0-1,8-9")), &roots);
        assert_eq!(multi.status(), 206);
        assert!(multi.headers()["Content-Type"].to_str().unwrap().starts_with("multipart/byteranges;"));
        assert!(String::from_utf8_lossy(multi.body()).contains("bytes 8-9/10\r\n\r\n89"));
        let invalid = respond(request(Some("bytes=99-")), &roots);
        assert_eq!(invalid.status(), 416);
        assert_eq!(invalid.headers()["Content-Range"], "bytes */10");
        let mut head = request(None);
        *head.method_mut() = tauri::http::Method::HEAD;
        let head = respond(head, &roots);
        assert_eq!(head.headers()["Content-Length"], "10");
        assert!(head.body().is_empty());
        #[cfg(unix)]
        {
            let outside = base.join("outside.png");
            std::fs::write(&outside, b"private").unwrap();
            std::fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink(&outside, &path).unwrap();
            assert_eq!(respond(request(None), &roots).status(), 403);
            std::fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink(&base, home.join("linked-dir")).unwrap();
            let linked = home.join("linked-dir/outside.png");
            assert_eq!(canonical_scoped_path(&linked, &roots), Err(403));
        }
        std::fs::remove_file(&path).ok();
        let missing = respond(request(None), &roots);
        assert_eq!(missing.status(), 404);
        assert!(missing.body().is_empty());
        std::fs::remove_dir_all(base).unwrap();
    }
}
