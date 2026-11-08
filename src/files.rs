//! The directory being walked through, and loading its files.

use crate::code_view::{self, Line};
use crate::highlight::{self, LineSpans};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::process::Command;

/// Files larger than this aren't shown: highlighting them would take too long.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct DirInfo {
    pub path: PathBuf,
    pub is_git: bool,
    pub branch: Option<String>,
}

async fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Any directory will do; being a git repository only adds information.
pub async fn verify_dir(path: PathBuf) -> Result<DirInfo, String> {
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    let path = tokio::fs::canonicalize(&path).await.map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    let is_git = git(&path, &["rev-parse", "--is-inside-work-tree"]).await.as_deref() == Some("true");
    let branch = if is_git { git(&path, &["branch", "--show-current"]).await.filter(|b| !b.is_empty()) } else { None };
    Ok(DirInfo { path, is_git, branch })
}

#[derive(Debug, Clone)]
pub struct LoadedFile {
    pub text: Arc<String>,
    pub lines: Vec<Line>,
    pub spans: Vec<LineSpans>,
}

fn text_or_none(bytes: Vec<u8>) -> Option<String> {
    if bytes.iter().take(8000).any(|&b| b == 0) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Loads and highlights a file; the error says why it can't be shown.
pub async fn load_file(root: PathBuf, rel: String, dark: bool) -> Result<LoadedFile, String> {
    let path = root.join(&rel);
    let size = tokio::fs::metadata(&path).await.map_err(|e| e.to_string())?.len();
    if size > MAX_FILE_BYTES {
        return Err(format!("the file is too large to show ({} KiB)", size / 1024));
    }
    let bytes = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;
    let text = text_or_none(bytes).ok_or("it is a binary file")?;
    if text.trim().is_empty() {
        return Err("the file is empty".into());
    }
    let lines = code_view::lines_of(&text);
    let text = Arc::new(text);
    let spans = {
        let text = text.clone();
        tokio::task::spawn_blocking(move || highlight::highlight_file(&rel, &text, dark)).await.map_err(|e| e.to_string())?
    };
    Ok(LoadedFile { text, lines, spans })
}
