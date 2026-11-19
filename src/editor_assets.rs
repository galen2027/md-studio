//! Read-only, embedded resources for the lazily loaded structured editor.

use std::borrow::Cow;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const SCHEME: &str = "mdpreview-editor";

pub fn base_url() -> &'static str {
    if cfg!(target_os = "windows") {
        "http://mdpreview-editor.localhost"
    } else {
        "mdpreview-editor://localhost"
    }
}

pub fn response(path: &str) -> wry::http::Response<Cow<'static, [u8]>> {
    let resource: Option<(&str, &'static [u8])> = match path {
        "/LICENSE" => Some((
            "text/plain",
            include_bytes!("../assets/editor/vditor/LICENSE"),
        )),
        "/LUTE-LICENSE" => Some((
            "text/plain",
            include_bytes!("../assets/editor/vditor/LUTE-LICENSE"),
        )),
        "/KATEX-LICENSE" => Some((
            "text/plain",
            include_bytes!("../assets/editor/vditor/KATEX-LICENSE"),
        )),
        "/dist/index.min.js" => Some((
            "text/javascript",
            include_bytes!("../assets/editor/vditor/dist/index.min.js"),
        )),
        "/dist/index.css" => Some((
            "text/css",
            include_bytes!("../assets/editor/vditor/dist/index.css"),
        )),
        "/dist/js/lute/lute.min.js" => Some((
            "text/javascript",
            include_bytes!("../assets/editor/vditor/dist/js/lute/lute.min.js"),
        )),
        "/dist/js/i18n/zh_CN.js" => Some((
            "text/javascript",
            include_bytes!("../assets/editor/vditor/dist/js/i18n/zh_CN.js"),
        )),
        "/dist/js/i18n/en_US.js" => Some((
            "text/javascript",
            include_bytes!("../assets/editor/vditor/dist/js/i18n/en_US.js"),
        )),
        "/dist/js/icons/ant.js" => Some((
            "text/javascript",
            include_bytes!("../assets/editor/vditor/dist/js/icons/ant.js"),
        )),
        "/dist/css/content-theme/light.css" => Some((
            "text/css",
            include_bytes!("../assets/editor/vditor/dist/css/content-theme/light.css"),
        )),
        "/dist/css/content-theme/dark.css" => Some((
            "text/css",
            include_bytes!("../assets/editor/vditor/dist/css/content-theme/dark.css"),
        )),
        "/dist/js/mermaid/mermaid.min.js" => Some((
            "text/javascript",
            include_bytes!("../assets/mermaid/mermaid.min.js"),
        )),
        "/dist/js/katex/katex.min.js" => Some((
            "text/javascript",
            include_bytes!("../assets/katex/katex.min.js"),
        )),
        "/dist/js/katex/katex.min.css" => Some((
            "text/css",
            include_bytes!("../assets/katex/katex.inline.css"),
        )),
        "/dist/js/katex/mhchem.min.js" => Some((
            "text/javascript",
            include_bytes!("../assets/editor/vditor/dist/js/katex/mhchem.min.js"),
        )),
        "/dist/js/highlight.js/styles/github.min.css" => {
            Some(("text/css", include_bytes!("../assets/hljs/github.min.css")))
        }
        "/dist/js/highlight.js/styles/github-dark.min.css" => Some((
            "text/css",
            include_bytes!("../assets/hljs/github-dark.min.css"),
        )),
        _ => None,
    };
    let (status, mime, bytes) = match resource {
        Some((mime, bytes)) => (200, mime, bytes),
        None => (404, "text/plain", &b"Unknown editor resource"[..]),
    };
    wry::http::Response::builder()
        .status(status)
        .header("Content-Type", mime)
        .header("Access-Control-Allow-Origin", "*")
        .header("X-Content-Type-Options", "nosniff")
        .header("Cache-Control", "no-cache")
        .body(Cow::Borrowed(bytes))
        .unwrap()
}

/// Create an exact, never-overwritten original before structured serialization.
pub fn backup_document(path: &Path, directory: &Path) -> std::io::Result<PathBuf> {
    use std::hash::{Hash, Hasher};
    let bytes = fs::read(path)?;
    fs::create_dir_all(directory)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hash);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_millis();
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let backup = directory.join(format!("{millis}-{:016x}-{name}.bak", hash.finish()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editor_resources_are_offline_and_scoped() {
        assert_eq!(response("/dist/index.min.js").status(), 200);
        assert_eq!(response("/dist/js/lute/lute.min.js").status(), 200);
        assert_eq!(response("/../Cargo.toml").status(), 404);
        assert_eq!(response("/C:/private.md").status(), 404);
    }
    #[test]
    fn backup_preserves_original_bytes_and_does_not_modify_source() {
        let directory = std::env::temp_dir().join(format!(
            "md-preview-rich-backup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("原文.md");
        let bytes = b"\xef\xbb\xbf# title\r\n\r\n**bold**\r\n";
        fs::write(&path, bytes).unwrap();
        let backup = backup_document(&path, &directory.join("backups")).unwrap();
        assert_eq!(fs::read(backup).unwrap(), bytes);
        assert_eq!(fs::read(path).unwrap(), bytes);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn missing_original_does_not_create_a_fake_backup() {
        let directory =
            std::env::temp_dir().join(format!("md-preview-missing-backup-{}", std::process::id()));
        let result = backup_document(&directory.join("missing.md"), &directory.join("backups"));
        assert!(result.is_err());
        assert!(!directory.join("backups").exists());
    }
}
