//! Content-addressed file storage with thumbnails.
//!
//! Files live at `files/ab/cd/<blake3>.<ext>`, so identical outputs are stored once.
//! Thumbnails are 512 px JPEGs at `thumbs/ab/<blake3>.jpg` (image resize or ffmpeg poster frame).

use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::anyhow;
use futures::StreamExt;
use pictesa_core::MediaKind;
use tokio::io::AsyncWriteExt;

pub const THUMB_EDGE: u32 = 512;
/// Refuse single files above this size (guards disk and memory against runaway responses).
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub struct FileStore {
    files: PathBuf,
    thumbs: PathBuf,
    tmp: PathBuf,
    ffmpeg: Option<PathBuf>,
    ffprobe: Option<PathBuf>,
}

/// What we learned about a stored file.
#[derive(Debug, Clone)]
pub struct Stored {
    pub hash: String,
    pub ext: String,
    pub mime: String,
    pub kind: MediaKind,
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub has_thumb: bool,
}

impl FileStore {
    pub fn new(data_dir: &Path) -> anyhow::Result<Self> {
        let s = Self {
            files: data_dir.join("files"),
            thumbs: data_dir.join("thumbs"),
            tmp: data_dir.join("tmp"),
            ffmpeg: pictesa_providers::local_video::binary("ffmpeg"),
            ffprobe: pictesa_providers::local_video::binary("ffprobe"),
        };
        for d in [&s.files, &s.thumbs, &s.tmp] {
            std::fs::create_dir_all(d)?;
        }
        // Leftovers from an interrupted download. Only old ones: another process (the server next to a
        // CLI command) may be writing fresh ones right now.
        if let Ok(entries) = std::fs::read_dir(&s.tmp) {
            let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(6 * 3600);
            for e in entries.flatten() {
                let old = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < cutoff);
                if old {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        Ok(s)
    }

    pub fn has_ffmpeg(&self) -> bool {
        self.ffmpeg.is_some() && self.ffprobe.is_some()
    }

    pub fn path(&self, hash: &str, ext: &str) -> PathBuf {
        self.files.join(&hash[0..2]).join(&hash[2..4]).join(format!("{hash}.{ext}"))
    }

    pub fn thumb(&self, hash: &str) -> PathBuf {
        self.thumbs.join(&hash[0..2]).join(format!("{hash}.jpg"))
    }

    fn tmp_file(&self) -> PathBuf {
        self.tmp.join(format!("{}.part", crate::new_id()))
    }

    /// Stores bytes. `trusted` (provider outputs) lets the declared type count when sniffing finds
    /// nothing; files from people (uploads, imports) must be recognised from their content.
    pub async fn store_bytes(&self, bytes: &[u8], mime_hint: Option<&str>, name_hint: Option<&str>, trusted: bool) -> anyhow::Result<Stored> {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(anyhow!("file is larger than 4 GB"));
        }
        let tmp = self.tmp_file();
        tokio::fs::write(&tmp, bytes).await?;
        let hash = blake3::hash(bytes).to_hex().to_string();
        let mime = decide_mime(bytes, mime_hint, name_hint, trusted);
        self.finalize(tmp, hash, bytes.len() as u64, mime).await
    }

    /// Streams an HTTP response to disk while hashing it.
    pub async fn store_response(&self, resp: reqwest::Response, name_hint: Option<&str>, trusted: bool) -> anyhow::Result<Stored> {
        let header_mime = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(';').next().unwrap_or(s).trim().to_string());
        let stream = resp.bytes_stream().map(|c| c.map_err(|e| std::io::Error::other(e.to_string())));
        self.store_stream(stream, header_mime.as_deref(), name_hint, trusted).await
    }

    /// Streams any byte source (an upload, a download) to disk while hashing it, never holding it in memory.
    pub async fn store_stream<S>(&self, mut stream: S, mime_hint: Option<&str>, name_hint: Option<&str>, trusted: bool) -> anyhow::Result<Stored>
    where
        S: futures::Stream<Item = Result<bytes::Bytes, std::io::Error>> + Unpin,
    {
        let tmp = self.tmp_file();
        let mut file = tokio::fs::File::create(&tmp).await?;
        let mut hasher = blake3::Hasher::new();
        let mut size = 0u64;
        let mut head: Vec<u8> = Vec::with_capacity(64);
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    drop(file);
                    let _ = tokio::fs::remove_file(&tmp).await;
                    return Err(anyhow!("transfer interrupted: {e}"));
                }
            };
            size += chunk.len() as u64;
            if size > MAX_FILE_BYTES {
                drop(file);
                let _ = tokio::fs::remove_file(&tmp).await;
                return Err(anyhow!("file is larger than 4 GB"));
            }
            if head.len() < 64 {
                head.extend_from_slice(&chunk[..chunk.len().min(64 - head.len())]);
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        drop(file);
        if size == 0 {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(anyhow!("The file is empty."));
        }
        let mime = decide_mime(&head, mime_hint, name_hint, trusted);
        self.finalize(tmp, hasher.finalize().to_hex().to_string(), size, mime).await
    }

    async fn finalize(&self, tmp: PathBuf, hash: String, size: u64, mime: Option<String>) -> anyhow::Result<Stored> {
        let Some(mime) = mime else {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(anyhow!("Pictesa can only use image, video and audio files, and this file is something else."));
        };
        let kind = MediaKind::from_mime(&mime).ok_or_else(|| {
            let _ = std::fs::remove_file(&tmp);
            anyhow!("unsupported file type {mime}")
        })?;
        let ext = ext_for(&mime).to_string();
        let dest = self.path(&hash, &ext);
        if dest.exists() {
            let _ = tokio::fs::remove_file(&tmp).await;
        } else {
            tokio::fs::create_dir_all(dest.parent().unwrap()).await?;
            tokio::fs::rename(&tmp, &dest).await?;
        }
        let mut stored = Stored { hash, ext, mime, kind, size, width: None, height: None, duration_ms: None, has_thumb: false };
        self.describe(&dest, &mut stored).await;
        Ok(stored)
    }

    /// Reads dimensions and duration and writes the thumbnail. Failures only lose the extras.
    async fn describe(&self, path: &Path, s: &mut Stored) {
        let thumb = self.thumb(&s.hash);
        match s.kind {
            MediaKind::Image => {
                let (src, dst) = (path.to_path_buf(), thumb.clone());
                let made = tokio::task::spawn_blocking(move || image_thumb(&src, &dst)).await;
                match made {
                    Ok(Ok((w, h))) => {
                        s.width = Some(w);
                        s.height = Some(h);
                        s.has_thumb = true;
                    }
                    Ok(Err(e)) => tracing::warn!("thumbnail failed for {}: {e:#}", s.hash),
                    Err(e) => tracing::warn!("thumbnail task failed: {e}"),
                }
            }
            MediaKind::Video => {
                if let Some((w, h, d)) = self.probe(path).await {
                    s.width = w;
                    s.height = h;
                    s.duration_ms = d;
                }
                s.has_thumb = self.video_thumb(path, &thumb, s.duration_ms).await;
            }
            MediaKind::Lora => {},
            MediaKind::Audio => {
                if let Some((_, _, d)) = self.probe(path).await {
                    s.duration_ms = d;
                }
            }
        }
    }

    async fn probe(&self, path: &Path) -> Option<(Option<u32>, Option<u32>, Option<u64>)> {
        let ffprobe = self.ffprobe.as_ref()?;
        let out = tokio::process::Command::new(ffprobe)
            .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
            .arg(path)
            .stdin(Stdio::null())
            .output()
            .await
            .ok()?;
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
        let video = v.get("streams")?.as_array()?.iter().find(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("video"));
        let w = video.and_then(|s| s.get("width")).and_then(|x| x.as_u64()).map(|x| x as u32);
        let h = video.and_then(|s| s.get("height")).and_then(|x| x.as_u64()).map(|x| x as u32);
        let d = v
            .pointer("/format/duration")
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .map(|s| (s * 1000.0) as u64);
        Some((w, h, d))
    }

    async fn video_thumb(&self, path: &Path, thumb: &Path, duration_ms: Option<u64>) -> bool {
        let Some(ffmpeg) = &self.ffmpeg else { return false };
        if let Some(parent) = thumb.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let at = duration_ms.map(|d| (d as f64 / 1000.0 * 0.15).min(1.0)).unwrap_or(0.0);
        let status = tokio::process::Command::new(ffmpeg)
            .args(["-v", "error", "-y", "-ss", &format!("{at:.2}"), "-i"])
            .arg(path)
            .args(["-frames:v", "1", "-vf", &format!("scale='min({THUMB_EDGE},iw)':-2"), "-q:v", "4"])
            .arg(thumb)
            .stdin(Stdio::null())
            .status()
            .await;
        matches!(status, Ok(s) if s.success()) && thumb.exists()
    }

    /// Removes a file and its thumbnail (caller checks no other asset uses the hash).
    pub async fn remove(&self, hash: &str, ext: &str) {
        let _ = tokio::fs::remove_file(self.path(hash, ext)).await;
        let _ = tokio::fs::remove_file(self.thumb(hash)).await;
    }
}

fn image_thumb(src: &Path, dst: &Path) -> anyhow::Result<(u32, u32)> {
    let img = image::ImageReader::open(src)?.with_guessed_format()?.decode()?;
    let (w, h) = (img.width(), img.height());
    let small = if w.max(h) > THUMB_EDGE { img.thumbnail(THUMB_EDGE, THUMB_EDGE) } else { img };
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = std::io::BufWriter::new(std::fs::File::create(dst)?);
    let rgb = if small.color().has_alpha() { on_checkerboard(&small.to_rgba8()) } else { small.to_rgb8() };
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82).encode_image(&rgb)?;
    Ok((w, h))
}

/// JPEG thumbnails have no transparency: cut-outs show on a light checkerboard instead of black.
fn on_checkerboard(img: &image::RgbaImage) -> image::RgbImage {
    image::RgbImage::from_fn(img.width(), img.height(), |x, y| {
        let [r, g, b, a] = img.get_pixel(x, y).0;
        let back: u32 = if (x / 16 + y / 16) % 2 == 0 { 0xf2 } else { 0xe2 };
        let mix = |c: u8| ((c as u32 * a as u32 + back * (255 - a as u32)) / 255) as u8;
        image::Rgb([mix(r), mix(g), mix(b)])
    })
}

/// Types Pictesa stores and serves. Nothing that a browser could execute (no SVG or HTML).
const SAFE_MIMES: &[&str] = &[
    "application/x-safetensors",
    "image/png", "image/jpeg", "image/webp", "image/gif", "image/avif", "image/heic",
    "video/mp4", "video/quicktime", "video/webm",
    "audio/mpeg", "audio/wav", "audio/x-wav", "audio/ogg", "audio/flac", "audio/mp4", "audio/aac",
];

/// Content sniffing first; a declared type only for trusted sources and only when it is a safe type.
fn decide_mime(head: &[u8], mime_hint: Option<&str>, name_hint: Option<&str>, trusted: bool) -> Option<String> {
    if let Some(m) = sniff(head) {
        return Some(m.to_string());
    }
    if !trusted {
        return None;
    }
    if trusted && name_hint.is_some_and(|n| n.split('?').next().unwrap_or(n).ends_with(".safetensors")) { return Some("application/x-safetensors".into()); }
    let declared = mime_hint
        .map(|m| m.split(';').next().unwrap_or(m).trim().to_ascii_lowercase())
        .filter(|m| m != "application/octet-stream" && m != "binary/octet-stream")
        .or_else(|| name_hint.and_then(|n| mime_guess::from_path(n.split('?').next().unwrap_or(n)).first_raw().map(String::from)));
    declared.filter(|m| SAFE_MIMES.contains(&m.as_str()))
}

/// Recognizes common media formats from their first bytes.
pub fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else if b.starts_with(b"GIF8") {
        Some("image/gif")
    } else if b.len() >= 12 && &b[4..8] == b"ftyp" {
        match &b[8..12] {
            b"avif" | b"avis" => Some("image/avif"),
            b"heic" | b"heix" | b"mif1" => Some("image/heic"),
            b"qt  " => Some("video/quicktime"),
            b"M4A " => Some("audio/mp4"),
            _ => Some("video/mp4"),
        }
    } else if b.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        Some("video/webm")
    } else if b.starts_with(b"ID3") || b.starts_with(&[0xFF, 0xFB]) || b.starts_with(&[0xFF, 0xF3]) {
        Some("audio/mpeg")
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WAVE" {
        Some("audio/wav")
    } else if b.starts_with(b"OggS") {
        Some("audio/ogg")
    } else if b.starts_with(b"fLaC") {
        Some("audio/flac")
    } else {
        None
    }
}

pub fn ext_for(mime: &str) -> &'static str {
    match mime {
        "application/x-safetensors" => "safetensors",
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/avif" => "avif",
        "image/heic" => "heic",
        "image/svg+xml" => "svg",
        "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        "video/webm" => "webm",
        "audio/mpeg" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        "audio/mp4" => "m4a",
        m if m.starts_with("image/") => "img",
        m if m.starts_with("video/") => "video",
        _ => "bin",
    }
}
