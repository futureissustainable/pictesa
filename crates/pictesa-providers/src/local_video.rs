//! Free, bounded ffmpeg tools. Inputs are library files resolved by the normal queue.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use async_trait::async_trait;
use pictesa_core::{
    Ctx, GenerationRequest, InputRole, InputSlot, MediaKind, ModelSpec, Output, ParamSpec, Pricing,
    Provider, ProviderError, ProviderInfo, ProviderKind, ProviderResult, Submission, Task,
};
use serde_json::Value;
use tokio::{io::AsyncReadExt, process::Command};

pub struct LocalVideo;
const MAX_SECONDS: f64 = 600.0;
const MAX_BYTES: u64 = 128 * 1024 * 1024;
const MAX_RUNTIME: Duration = Duration::from_secs(600);

/// Shared executable lookup, including the existing bundled-ffmpeg override.
pub fn binary(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.into()
    };
    let bundled = std::env::var_os("PICTESA_FFMPEG_DIR").map(PathBuf::from);
    let paths = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    bundled
        .into_iter()
        .chain(paths)
        .map(|dir| dir.join(&exe))
        .find(|path| path.is_file() && executable(path))
}

pub fn available() -> bool {
    binary("ffmpeg").is_some() && binary("ffprobe").is_some()
}

fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[async_trait]
impl Provider for LocalVideo {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "local".into(),
            name: "Local video (ffmpeg)".into(),
            kind: ProviderKind::Local,
            fields: vec![],
            signup_url: None,
            docs_url: None,
            max_concurrency: 1,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        let seconds = |key, label, default| {
            ParamSpec::float(key, label, 0.0, MAX_SECONDS, 0.01).default_value(default)
        };
        let model = |id, name, output| {
            let mut m = ModelSpec::new("local", id, name, output, &[Task::VideoToVideo])
                .input(InputSlot::new(InputRole::Video, MediaKind::Video, 1).required());
            m.prompt = false;
            m.pricing = Some(Pricing::Local);
            m
        };
        let mut join = model("join", "Join clips", MediaKind::Video);
        join.inputs[0].min = 2;
        join.inputs[0].max = 10;
        let mut smooth = model("smooth", "Smooth frames", MediaKind::Video);
        smooth.description = Some("Interpolate to twice the frame rate. Slow on CPU.".into());
        vec![
            model("trim", "Trim video", MediaKind::Video).params([
                seconds("start", "In (seconds)", 0.0),
                seconds("end", "Out (seconds)", 1.0),
            ]),
            join,
            model("frame", "Grab frame", MediaKind::Image).param(seconds(
                "time",
                "Time (seconds)",
                0.0,
            )),
            smooth,
            model("add-audio", "Add music or sound", MediaKind::Video)
                .input(InputSlot::new(InputRole::Audio, MediaKind::Audio, 1).required())
                .params([
                    ParamSpec::choices("mode", "Original audio", &["replace", "mix"])
                        .default_value("replace"),
                    ParamSpec::float("volume", "Added audio volume", 0.0, 4.0, 0.05)
                        .default_value(1.0),
                    seconds("fade_in", "Fade in (seconds)", 0.0),
                    seconds("fade_out", "Fade out (seconds)", 0.0),
                ]),
        ]
    }
    async fn check(&self, _ctx: &Ctx) -> ProviderResult<()> {
        if available() {
            Ok(())
        } else {
            Err(ProviderError::NotSupported(
                "Install ffmpeg and ffprobe on PATH to use local video tools.".into(),
            ))
        }
    }
    async fn submit(
        &self,
        _ctx: &Ctx,
        _model: &ModelSpec,
        req: &GenerationRequest,
    ) -> ProviderResult<Submission> {
        if req.inputs.is_empty() || req.inputs.len() > 10 {
            return Err(invalid("Choose at most 10 input files."));
        }
        let temp = TempDir::new()?;
        let mut probes = Vec::new();
        for input in &req.inputs {
            probes.push(probe(&input.path).await?);
        }
        let output = temp.0.join(if req.model == "frame" {
            "frame.png"
        } else {
            "video.mp4"
        });
        let args = arguments(req, &probes, &output)?;
        let mut command =
            Command::new(binary("ffmpeg").ok_or_else(|| invalid("Install ffmpeg on PATH."))?);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let status = tokio::time::timeout(MAX_RUNTIME, command.status())
            .await
            .map_err(|_| {
                ProviderError::Failed(
                    "Local video tool exceeded the 10 minute runtime limit.".into(),
                )
            })?
            .map_err(|_| {
                ProviderError::Failed("Could not start ffmpeg. Install it on PATH.".into())
            })?;
        if !status.success() {
            return Err(ProviderError::Failed("ffmpeg could not process these files. Check that the media is valid and the encoder supports it.".into()));
        }
        let size = tokio::fs::metadata(&output).await.map_err(io_error)?.len();
        if size == 0 || size >= MAX_BYTES {
            return Err(ProviderError::InvalidInput(
                "Output exceeds the 128 MiB limit. Use shorter or smaller clips.".into(),
            ));
        }
        let data = tokio::fs::read(output).await.map_err(io_error)?;
        Ok(Submission::Done(vec![Output::Bytes {
            data,
            mime: if req.model == "frame" {
                "image/png"
            } else {
                "video/mp4"
            }
            .into(),
        }]))
    }
}

fn io_error(_: std::io::Error) -> ProviderError {
    ProviderError::Failed("Could not read or write local video files.".into())
}
fn invalid(message: &str) -> ProviderError {
    ProviderError::InvalidInput(message.into())
}
struct TempDir(PathBuf);
impl TempDir {
    fn new() -> ProviderResult<Self> {
        let path = std::env::temp_dir().join(format!("pictesa-video-{}", ulid::Ulid::new()));
        std::fs::create_dir(&path).map_err(io_error)?;
        Ok(Self(path))
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, Clone)]
struct Probe {
    width: u32,
    height: u32,
    fps: f64,
    duration: f64,
    audio: bool,
}

async fn probe(path: &Path) -> ProviderResult<Probe> {
    let mut command =
        Command::new(binary("ffprobe").ok_or_else(|| invalid("Install ffprobe on PATH."))?);
    command
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-format_whitelist",
            "mov,matroska,webm,avi,mpeg,mpegts,mp3,wav,ogg,flac,aac",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| invalid("Install ffmpeg and ffprobe on PATH."))?;
    let mut stdout = child.stdout.take().unwrap().take(65537);
    let mut bytes = Vec::new();
    let read = async {
        stdout.read_to_end(&mut bytes).await.map_err(io_error)?;
        if bytes.len() > 65536 {
            return Err(invalid("Media metadata exceeds the limit."));
        }
        if !child.wait().await.map_err(io_error)?.success() {
            return Err(invalid("Cannot read this media file."));
        }
        serde_json::from_slice::<Value>(&bytes).map_err(|_| invalid("Cannot read this media file."))
    };
    let value = tokio::time::timeout(Duration::from_secs(15), read)
        .await
        .map_err(|_| invalid("Media probe timed out."))??;
    parse_probe(&value)
}

fn parse_probe(value: &Value) -> ProviderResult<Probe> {
    let streams = value["streams"]
        .as_array()
        .ok_or_else(|| invalid("Media has no streams."))?;
    let video = streams.iter().find(|s| s["codec_type"] == "video");
    let mut width = video.and_then(|s| s["width"].as_u64()).unwrap_or(0) as u32;
    let mut height = video.and_then(|s| s["height"].as_u64()).unwrap_or(0) as u32;
    let rotation = video
        .and_then(|s| s["side_data_list"].as_array())
        .and_then(|list| list.iter().find_map(|item| item["rotation"].as_i64()))
        .unwrap_or(0);
    if rotation.rem_euclid(180) == 90 {
        std::mem::swap(&mut width, &mut height);
    }
    let fps = video
        .and_then(|s| s["avg_frame_rate"].as_str())
        .and_then(|s| s.split_once('/'))
        .map(|(n, d)| n.parse::<f64>().unwrap_or(0.0) / d.parse::<f64>().unwrap_or(0.0))
        .unwrap_or(0.0);
    let duration = value["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    if !duration.is_finite() || duration <= 0.0 || duration > MAX_SECONDS {
        return Err(invalid(
            "Media must be no longer than 10 minutes and have a known duration.",
        ));
    }
    if video.is_some()
        && (width < 2
            || height < 2
            || width > 3840
            || height > 3840
            || width as u64 * height as u64 > 3840 * 2160
            || !fps.is_finite()
            || fps <= 0.0
            || fps > 120.0)
    {
        return Err(invalid("Video must be at most 4K and 120 fps."));
    }
    Ok(Probe {
        width,
        height,
        fps,
        duration,
        audio: streams.iter().any(|s| s["codec_type"] == "audio"),
    })
}

/// Pure argument construction: no user text is interpreted by a shell or filter graph.
fn arguments(
    req: &GenerationRequest,
    probes: &[Probe],
    output: &Path,
) -> ProviderResult<Vec<OsString>> {
    let videos: Vec<_> = req
        .inputs
        .iter()
        .enumerate()
        .filter(|(_, i)| i.role == InputRole::Video && i.kind == MediaKind::Video)
        .collect();
    let audio = req
        .inputs
        .iter()
        .position(|i| i.role == InputRole::Audio && i.kind == MediaKind::Audio);
    if probes.len() != req.inputs.len()
        || videos.is_empty()
        || videos.len() > 10
        || req.inputs.len() > 11
    {
        return Err(invalid("Choose one video, or 2 to 10 clips for join."));
    }
    let first_index = videos[0].0;
    let first = &probes[first_index];
    if videos.iter().any(|(i, _)| probes[*i].width == 0) {
        return Err(invalid("A video input has no video stream."));
    }
    if req.model != "join" && videos.len() != 1 {
        return Err(invalid("This tool takes one video."));
    }
    let number = |key: &str, default: f64, max: f64| -> ProviderResult<f64> {
        let n = match req.params.get(key) {
            None => default,
            Some(v) => v
                .as_f64()
                .ok_or_else(|| invalid("Settings must be numbers."))?,
        };
        if n.is_finite() && n >= 0.0 && n <= max {
            Ok(n)
        } else {
            Err(invalid("A setting is outside the supported range."))
        }
    };
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-y",
        "-threads",
        "2",
        "-filter_threads",
        "2",
        "-filter_complex_threads",
        "2",
    ]
    .iter()
    .map(Into::into)
    .collect();
    for input in &req.inputs {
        args.extend([
            "-threads".into(),
            "2".into(),
            "-protocol_whitelist".into(),
            "file,pipe".into(),
            "-format_whitelist".into(),
            "mov,matroska,webm,avi,mpeg,mpegts,mp3,wav,ogg,flac,aac".into(),
            "-i".into(),
            input.path.as_os_str().into(),
        ]);
    }
    let mut add = |values: Vec<String>| args.extend(values.into_iter().map(OsString::from));
    let video_map = format!("{first_index}:v:0");
    let audio_map = format!("{first_index}:a:0?");
    match req.model.as_str() {
        "trim" => {
            let start = number("start", 0.0, MAX_SECONDS)?;
            let end = number("end", 1.0, MAX_SECONDS)?;
            if end <= start || end > first.duration + 0.001 {
                return Err(invalid("Out must follow In and stay within the clip."));
            }
            add(vec![
                "-ss".into(),
                start.to_string(),
                "-t".into(),
                (end - start).to_string(),
                "-map".into(),
                video_map,
                "-map".into(),
                audio_map,
            ]);
        }
        "frame" => {
            let time = number("time", 0.0, MAX_SECONDS)?;
            if time >= first.duration {
                return Err(invalid("Choose a time before the end of the video."));
            }
            add(vec![
                "-ss".into(),
                time.to_string(),
                "-map".into(),
                video_map,
                "-frames:v".into(),
                "1".into(),
                "-an".into(),
            ]);
        }
        "smooth" => {
            if first.fps > 60.0 {
                return Err(invalid(
                    "Smooth frames accepts up to 60 fps and outputs up to 120 fps.",
                ));
            }
            add(vec!["-map".into(), video_map, "-map".into(), audio_map, "-vf".into(), format!("tpad=stop_mode=clone:stop_duration=1,minterpolate=fps={}:mi_mode=mci,scale=trunc(iw/2)*2:trunc(ih/2)*2", first.fps * 2.0), "-t".into(), first.duration.to_string()]);
        }
        "join" => {
            if videos.len() < 2 {
                return Err(invalid("Join needs 2 to 10 clips."));
            }
            let duration: f64 = videos.iter().map(|(i, _)| probes[*i].duration).sum();
            if duration > MAX_SECONDS {
                return Err(invalid("Joined clips must total no more than 10 minutes."));
            }
            let (width, height) = (first.width / 2 * 2, first.height / 2 * 2);
            let mut filters = Vec::new();
            let mut concat = String::new();
            for (n, (i, _)) in videos.iter().enumerate() {
                let p = &probes[*i];
                filters.push(format!("[{i}:v:0]setpts=PTS-STARTPTS,scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={},format=yuv420p[v{n}]", first.fps));
                let source = if p.audio {
                    format!("[{i}:a:0]aresample=48000,aformat=channel_layouts=stereo,asetpts=PTS-STARTPTS,apad")
                } else {
                    "anullsrc=r=48000:cl=stereo".into()
                };
                filters.push(format!("{source},atrim=duration={}[a{n}]", p.duration));
                concat.push_str(&format!("[v{n}][a{n}]"));
            }
            filters.push(format!("{concat}concat=n={}:v=1:a=1[v][a]", videos.len()));
            add(vec![
                "-filter_complex".into(),
                filters.join(";"),
                "-map".into(),
                "[v]".into(),
                "-map".into(),
                "[a]".into(),
                "-t".into(),
                duration.to_string(),
            ]);
        }
        "add-audio" => {
            let index = audio.ok_or_else(|| invalid("Choose an audio file."))?;
            if !probes[index].audio {
                return Err(invalid("The audio input has no audio stream."));
            }
            let volume = number("volume", 1.0, 4.0)?;
            let fade_in = number("fade_in", 0.0, first.duration)?;
            let fade_out = number("fade_out", 0.0, first.duration)?;
            let mode = req.str("mode").unwrap_or("replace");
            if mode != "replace" && mode != "mix" {
                return Err(invalid("Audio mode must be replace or mix."));
            }
            let mut filter = format!("[{index}:a:0]aresample=48000,aformat=channel_layouts=stereo,asetpts=PTS-STARTPTS,volume={volume},apad,atrim=duration={}", first.duration);
            if fade_in > 0.0 {
                filter.push_str(&format!(",afade=t=in:st=0:d={fade_in}"));
            }
            if fade_out > 0.0 {
                filter.push_str(&format!(
                    ",afade=t=out:st={}:d={fade_out}",
                    first.duration - fade_out
                ));
            }
            if mode == "mix" && first.audio {
                filter.push_str(&format!("[added];[{first_index}:a:0]asetpts=PTS-STARTPTS[original];[original][added]amix=inputs=2:duration=longest:normalize=0[a]"));
            } else {
                filter.push_str("[a]");
            }
            add(vec![
                "-filter_complex".into(),
                filter,
                "-map".into(),
                video_map,
                "-map".into(),
                "[a]".into(),
                "-t".into(),
                first.duration.to_string(),
            ]);
        }
        _ => return Err(invalid("Unknown local video tool.")),
    }
    if req.model != "frame" {
        if req.model != "smooth" && req.model != "join" {
            args.extend(
                ["-vf", "scale=trunc(iw/2)*2:trunc(ih/2)*2"]
                    .iter()
                    .map(OsString::from),
            );
        }
        args.extend(
            [
                "-c:v",
                "libx264",
                "-preset",
                "fast",
                "-crf",
                "20",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-movflags",
                "+faststart",
            ]
            .iter()
            .map(OsString::from),
        );
    }
    args.extend([
        "-threads".into(),
        "2".into(),
        "-fs".into(),
        MAX_BYTES.to_string().into(),
        output.as_os_str().into(),
    ]);
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pictesa_core::MediaInput;
    use serde_json::json;

    fn request(tool: &str, params: Value, kinds: &[MediaKind]) -> GenerationRequest {
        GenerationRequest {
            job_id: "test".into(),
            model: tool.into(),
            prompt: String::new(),
            negative_prompt: None,
            params: params.as_object().unwrap().clone(),
            num_outputs: 1,
            inputs: kinds
                .iter()
                .enumerate()
                .map(|(n, kind)| MediaInput {
                    role: if *kind == MediaKind::Audio {
                        InputRole::Audio
                    } else {
                        InputRole::Video
                    },
                    kind: *kind,
                    mime: format!("{}/mp4", kind.as_str()),
                    path: PathBuf::from(format!("/library/clip {n};$(echo bad).mp4")),
                    url: None,
                    width: None,
                    height: None,
                })
                .collect(),
        }
    }
    fn video() -> Probe {
        Probe {
            width: 96,
            height: 64,
            fps: 8.0,
            duration: 2.0,
            audio: true,
        }
    }

    #[test]
    fn arguments_validate_ranges_and_keep_paths_literal() {
        let p = video();
        let out = Path::new("/tmp/output.mp4");
        for params in [
            json!({"start":1,"end":1}),
            json!({"start":0,"end":3}),
            json!({"start":-1}),
            json!({"start":"0;evil"}),
        ] {
            assert!(arguments(
                &request("trim", params, &[MediaKind::Video]),
                &[p.clone()],
                out
            )
            .is_err());
        }
        assert!(arguments(
            &request("frame", json!({"time":2}), &[MediaKind::Video]),
            &[p.clone()],
            out
        )
        .is_err());
        assert!(arguments(
            &request("join", json!({}), &[MediaKind::Video]),
            &[p.clone()],
            out
        )
        .is_err());
        assert!(arguments(
            &request("add-audio", json!({}), &[MediaKind::Video]),
            &[p.clone()],
            out
        )
        .is_err());
        assert!(arguments(
            &request(
                "add-audio",
                json!({"mode":"oops"}),
                &[MediaKind::Video, MediaKind::Audio]
            ),
            &[p.clone(), p.clone()],
            out
        )
        .is_err());
        let req = request("trim", json!({"start":0.25,"end":1.5}), &[MediaKind::Video]);
        let args = arguments(&req, &[p.clone()], out).unwrap();
        let input = args.iter().position(|a| a == "-i").unwrap() + 1;
        assert_eq!(args[input], req.inputs[0].path.as_os_str());
        assert!(args.windows(2).any(|v| v[0] == "-t" && v[1] == "1.25"));
        let mut long = p.clone();
        long.duration = 400.0;
        assert!(arguments(
            &request("join", json!({}), &[MediaKind::Video, MediaKind::Video]),
            &[long.clone(), long],
            out
        )
        .is_err());
    }

    #[test]
    fn metadata_rejects_unbounded_work() {
        for (duration, width, fps) in [
            ("NaN", 96, "8/1"),
            ("601", 96, "8/1"),
            ("2", 8000, "8/1"),
            ("2", 96, "121/1"),
            ("2", 96, "0/0"),
        ] {
            let value = json!({"format":{"duration":duration},"streams":[{"codec_type":"video","width":width,"height":64,"avg_frame_rate":fps}]});
            assert!(parse_probe(&value).is_err(), "{value}");
        }
    }

    async fn fixture(path: &Path, source: &str, audio: bool) {
        let mut cmd = Command::new(binary("ffmpeg").unwrap());
        cmd.args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", source]);
        if audio {
            cmd.args([
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=2",
            ]);
        }
        cmd.args([
            "-t", "2", "-threads", "2", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac",
        ])
        .arg(path);
        assert!(cmd.status().await.unwrap().success());
    }

    async fn render(
        tool: &str,
        params: Value,
        paths: &[(&Path, MediaKind)],
        dest: &Path,
    ) -> Vec<u8> {
        let mut req = request(
            tool,
            params,
            &paths.iter().map(|(_, kind)| *kind).collect::<Vec<_>>(),
        );
        for (input, (path, _)) in req.inputs.iter_mut().zip(paths) {
            input.path = path.to_path_buf();
        }
        let ctx = Ctx {
            http: reqwest::Client::new(),
            creds: Default::default(),
        };
        let result = LocalVideo
            .submit(&ctx, &LocalVideo.models()[0], &req)
            .await
            .unwrap();
        let Submission::Done(mut outputs) = result else {
            panic!("expected local output")
        };
        let Output::Bytes { data, .. } = outputs.remove(0) else {
            panic!("expected bytes")
        };
        tokio::fs::write(dest, &data).await.unwrap();
        data
    }

    async fn rms(path: &Path, start: &str, duration: &str) -> f64 {
        let out = Command::new(binary("ffmpeg").unwrap())
            .args(["-v", "error", "-i"])
            .arg(path)
            .args([
                "-ss", start, "-t", duration, "-map", "0:a:0", "-ac", "1", "-ar", "8000", "-f",
                "f32le", "pipe:1",
            ])
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
        let samples: Vec<_> = out
            .stdout
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()) as f64)
            .collect();
        assert!(!samples.is_empty());
        (samples.iter().map(|v| v * v).sum::<f64>() / samples.len() as f64).sqrt()
    }

    #[tokio::test]
    async fn ffmpeg_tools_produce_media_with_timing_audio_and_normalized_join() {
        if !available() {
            eprintln!("skipping real media test: ffmpeg/ffprobe missing");
            return;
        }
        let dir = TempDir::new().unwrap();
        let a = dir.0.join("first clip.mp4");
        let b = dir.0.join("silent.mp4");
        let sound = dir.0.join("sound.wav");
        fixture(&a, "testsrc2=size=96x64:rate=8:duration=2", true).await;
        fixture(&b, "testsrc2=size=80x80:rate=12:duration=2", false).await;
        assert!(Command::new(binary("ffmpeg").unwrap())
            .args([
                "-v",
                "error",
                "-nostdin",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:sample_rate=48000:duration=1",
                "-threads",
                "2"
            ])
            .arg(&sound)
            .status()
            .await
            .unwrap()
            .success());
        let out = dir.0.join("result.mp4");
        render(
            "trim",
            json!({"start":0.5,"end":1.5}),
            &[(&a, MediaKind::Video)],
            &out,
        )
        .await;
        let p = probe(&out).await.unwrap();
        assert!((p.duration - 1.0).abs() < 0.15 && p.audio);
        render(
            "join",
            json!({}),
            &[(&a, MediaKind::Video), (&b, MediaKind::Video)],
            &out,
        )
        .await;
        let p = probe(&out).await.unwrap();
        assert_eq!((p.width, p.height, p.fps), (96, 64, 8.0));
        assert!((p.duration - 4.0).abs() < 0.15 && p.audio);
        assert!(rms(&out, "0.2", "0.5").await > 0.01);
        assert!(rms(&out, "2.5", "0.5").await < 0.001);
        let png = dir.0.join("frame.png");
        let first = render("frame", json!({"time":0}), &[(&a, MediaKind::Video)], &png).await;
        let later = render("frame", json!({"time":1}), &[(&a, MediaKind::Video)], &png).await;
        assert_eq!(
            (
                image::load_from_memory(&later).unwrap().width(),
                image::load_from_memory(&later).unwrap().height()
            ),
            (96, 64)
        );
        assert_ne!(
            first, later,
            "frame extraction follows the requested playhead"
        );
        render("smooth", json!({}), &[(&a, MediaKind::Video)], &out).await;
        let p = probe(&out).await.unwrap();
        assert_eq!(p.fps, 16.0);
        assert!((p.duration - 2.0).abs() < 0.15 && p.audio);
        render(
            "add-audio",
            json!({"mode":"replace","volume":0}),
            &[(&a, MediaKind::Video), (&sound, MediaKind::Audio)],
            &out,
        )
        .await;
        assert!(
            rms(&out, "0", "1").await < 0.001,
            "replace with volume zero removes original sound"
        );
        render(
            "add-audio",
            json!({"mode":"mix","volume":0}),
            &[(&a, MediaKind::Video), (&sound, MediaKind::Audio)],
            &out,
        )
        .await;
        assert!(
            rms(&out, "0", "1").await > 0.01,
            "mix preserves the original sound"
        );
        render(
            "add-audio",
            json!({"mode":"replace","fade_in":0.5,"fade_out":0.5}),
            &[(&b, MediaKind::Video), (&sound, MediaKind::Audio)],
            &out,
        )
        .await;
        let p = probe(&out).await.unwrap();
        assert!(p.audio && (p.duration - 2.0).abs() < 0.15);
        assert!(rms(&out, "0", "0.1").await < rms(&out, "0.6", "0.2").await * 0.4);
        assert!(
            rms(&out, "1.5", "0.4").await < 0.001,
            "short audio is padded to video duration"
        );
        assert!(Command::new(binary("ffmpeg").unwrap())
            .args([
                "-v",
                "error",
                "-nostdin",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:sample_rate=48000:duration=3",
                "-threads",
                "2"
            ])
            .arg(&sound)
            .status()
            .await
            .unwrap()
            .success());
        render(
            "add-audio",
            json!({"mode":"replace","fade_out":0.5}),
            &[(&b, MediaKind::Video), (&sound, MediaKind::Audio)],
            &out,
        )
        .await;
        assert!(
            rms(&out, "1.9", "0.1").await < rms(&out, "1.2", "0.2").await * 0.4,
            "fade-out lowers sound before video ends"
        );
    }
}
