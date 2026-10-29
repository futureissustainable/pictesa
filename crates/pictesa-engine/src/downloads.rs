//! The ComfyUI model manager: downloads the model files a template needs into the ComfyUI
//! models folder, resuming partial files, then refreshes the catalog so the model is ready.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::{Engine, EngineError, Result};

/// One file of a model download, as the studio and the CLI show it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Download {
    pub model: String,
    /// `<directory>/<name>` inside the models folder.
    pub file: String,
    pub done: u64,
    pub total: Option<u64>,
    /// `queued`, `running`, `done` or `failed`.
    pub status: &'static str,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelFile {
    name: String,
    #[serde(default)]
    directory: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    sha256: Option<String>,
}

/// A file name or folder from a template: plain segments only, so a template cannot write
/// outside the models folder.
fn safe_segments(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|s| {
            !s.is_empty() && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_. ".contains(c))
        })
}

/// True when `name` already sits in `dir` or a subfolder of it (ComfyUI finds those too).
fn present(dir: &Path, name: &str, depth: usize) -> bool {
    if dir.join(name).is_file() {
        return true;
    }
    depth > 0
        && std::fs::read_dir(dir).is_ok_and(|entries| {
            entries.flatten().any(|e| e.file_type().is_ok_and(|t| t.is_dir()) && present(&e.path(), name, depth - 1))
        })
}

impl Engine {
    /// Downloads progress since this process started, newest last.
    pub fn downloads(&self) -> Vec<Download> {
        self.inner.model_downloads.lock().unwrap().clone()
    }

    /// Starts downloading the files a ComfyUI template lacks into `models_dir`, or into the
    /// ComfyUI provider's models folder. Returns the files it queued (empty when none are missing).
    pub async fn start_model_download(&self, user: &str, model: &str, models_dir: Option<PathBuf>) -> Result<Vec<Download>> {
        // A fresh data folder (a CLI run on the ComfyUI machine) has not listed the templates yet.
        let spec = match self.model(user, model).await {
            Err(EngineError::NotFound(_)) if model.starts_with("comfyui:") => {
                self.refresh_models_for(user, "comfyui").await?;
                self.model(user, model).await?
            }
            other => other?,
        };
        let files: Vec<ModelFile> = serde_json::from_value(spec.meta.get("models").cloned().unwrap_or_default()).unwrap_or_default();
        if spec.provider != "comfyui" || files.is_empty() {
            return Err(EngineError::invalid("This model lists no model files to download.", None));
        }
        let provider = self.inner.catalog.provider("comfyui").ok_or_else(|| EngineError::not_found("provider"))?;
        let values = self.resolve_fields(user, provider.as_ref()).await?.values;
        let setting = |key: &str| values.get(key).map(|(v, _)| v.trim().to_string()).filter(|v| !v.is_empty());
        let dir = models_dir.or_else(|| setting("models_dir").map(PathBuf::from)).ok_or_else(|| {
            EngineError::invalid(
                "Set the ComfyUI models folder in Settings > Providers > ComfyUI, or run `pictesa download <model>` on the ComfyUI machine.",
                Some("models_dir"),
            )
        })?;
        if !dir.is_dir() {
            return Err(EngineError::invalid(format!("The ComfyUI models folder {} does not exist.", dir.display()), Some("models_dir")));
        }
        let token = setting("hf_token");

        let mut queued = Vec::new();
        let mut jobs = Vec::new();
        {
            let mut state = self.inner.model_downloads.lock().unwrap();
            for f in files {
                let folder = f.directory.clone().unwrap_or_default();
                let (Some(url), true) = (f.url.clone(), safe_segments(&f.name) && (folder.is_empty() || safe_segments(&folder))) else {
                    continue;
                };
                if !url.starts_with("https://") || present(&dir.join(&folder), &f.name, 3) {
                    continue;
                }
                let file = if folder.is_empty() { f.name.clone() } else { format!("{folder}/{}", f.name) };
                if state.iter().any(|d| d.file == file && matches!(d.status, "queued" | "running")) {
                    continue;
                }
                state.retain(|d| d.file != file);
                let entry = Download { model: model.to_string(), file: file.clone(), done: 0, total: f.size, status: "queued", error: None };
                state.push(entry.clone());
                queued.push(entry);
                jobs.push((file, url, dir.join(&folder).join(&f.name), f.size, f.sha256));
            }
        }
        let engine = self.clone();
        let (user, model) = (user.to_string(), model.to_string());
        tokio::spawn(async move {
            for (file, url, dest, size, sha256) in jobs {
                engine.set_download(&file, |d| d.status = "running");
                if let Err(e) = engine.fetch(&file, &url, &dest, size, sha256.as_deref(), token.as_deref()).await {
                    tracing::warn!("model download {file} failed: {e}");
                    engine.set_download(&file, |d| {
                        d.status = "failed";
                        d.error = Some(e.clone());
                    });
                    continue;
                }
                engine.set_download(&file, |d| d.status = "done");
            }
            if let Err(e) = engine.refresh_models_for(&user, "comfyui").await {
                tracing::debug!("refresh after downloading {model}: {e}");
            }
        });
        Ok(queued)
    }

    fn set_download(&self, file: &str, change: impl FnOnce(&mut Download)) {
        if let Some(d) = self.inner.model_downloads.lock().unwrap().iter_mut().find(|d| d.file == file) {
            change(d);
        }
    }

    /// Streams one file to `<dest>.part`, resuming a partial file, then renames it into place.
    async fn fetch(&self, file: &str, url: &str, dest: &Path, size: Option<u64>, sha256: Option<&str>, token: Option<&str>) -> std::result::Result<(), String> {
        let client = reqwest::Client::builder()
            .user_agent(format!("pictesa/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(120))
            .no_gzip()
            .build()
            .map_err(|e| e.to_string())?;
        let part = dest.with_file_name(format!("{}.part", dest.file_name().and_then(|n| n.to_str()).unwrap_or("download")));
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let mut start = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
        let mut req = client.get(url);
        if start > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={start}-"));
        }
        // reqwest drops this header when Hugging Face redirects to its file CDN.
        if let (Some(token), true) = (token, url.starts_with("https://huggingface.co/")) {
            req = req.bearer_auth(token);
        }
        let mut resp = req.send().await.map_err(|e| format!("cannot reach {url}: {e}"))?;
        let status = resp.status();
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && size == Some(start) {
            return finish(&part, dest, sha256).await;
        }
        if matches!(status.as_u16(), 401 | 403) {
            return Err(format!(
                "{url} refused the download ({status}). For a gated Hugging Face model, accept its license on huggingface.co and set a Hugging Face token in Settings > Providers > ComfyUI."
            ));
        }
        if !status.is_success() {
            return Err(format!("{url} answered {status}"));
        }
        if status != reqwest::StatusCode::PARTIAL_CONTENT {
            start = 0;
        }
        let total = size.or(resp.content_length().map(|n| n + start));
        let mut out = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(start > 0)
            .truncate(start == 0)
            .open(&part)
            .await
            .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
        let mut done = start;
        self.set_download(file, |d| {
            d.done = done;
            d.total = total;
        });
        while let Some(chunk) = resp.chunk().await.map_err(|e| format!("download of {url} broke off: {e}"))? {
            out.write_all(&chunk).await.map_err(|e| format!("cannot write {}: {e}", part.display()))?;
            done += chunk.len() as u64;
            self.set_download(file, |d| d.done = done);
        }
        out.flush().await.map_err(|e| e.to_string())?;
        drop(out);
        if let Some(expected) = size.filter(|&n| n != done) {
            return Err(format!("{file} has {done} bytes, the template expects {expected}; download it again"));
        }
        finish(&part, dest, sha256).await
    }
}

/// Checks the template's SHA-256 when it has one, then moves the finished file into place. A file
/// with the wrong hash is deleted so the next attempt starts clean.
async fn finish(part: &Path, dest: &Path, sha256: Option<&str>) -> std::result::Result<(), String> {
    if let Some(expected) = sha256 {
        let path = part.to_path_buf();
        let actual = tokio::task::spawn_blocking(move || -> std::io::Result<String> {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            std::io::copy(&mut std::fs::File::open(path)?, &mut hasher)?;
            Ok(format!("{:x}", hasher.finalize()))
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("cannot read {}: {e}", part.display()))?;
        if !actual.eq_ignore_ascii_case(expected) {
            let _ = tokio::fs::remove_file(part).await;
            return Err(format!("{} does not match the template's SHA-256; it was deleted, download it again", dest.display()));
        }
    }
    tokio::fs::rename(part, dest).await.map_err(|e| format!("cannot move {} into place: {e}", part.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_paths_stay_inside_the_models_folder() {
        assert!(safe_segments("diffusion_models"));
        assert!(safe_segments("flux3-image-klein-int8-convrot.safetensors"));
        assert!(safe_segments("checkpoints/sd15"));
        for bad in ["..", "../secrets", "a/../b", "/etc/passwd", "C:\\models", "", "a//b"] {
            assert!(!safe_segments(bad), "{bad}");
        }
    }
}
