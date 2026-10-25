//! ai-toolkit runs one subprocess at a time through the ordinary provider queue.
use async_trait::async_trait;
use pictesa_core::{
    lora::{progress, TrainingOptions},
    *,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, OnceLock},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

pub struct AiToolkit;
const MIME: &str = "application/x-safetensors";
struct Run {
    task: tokio::task::JoinHandle<()>,
    state: Arc<Mutex<State>>,
}
#[derive(Default)]
struct State {
    progress: Option<f32>,
    result: Option<Result<(PathBuf, PathBuf), String>>,
}
fn runs() -> &'static Mutex<HashMap<String, Run>> {
    static RUNS: OnceLock<Mutex<HashMap<String, Run>>> = OnceLock::new();
    RUNS.get_or_init(Default::default)
}
/// Only disposable training files live here. Successful output ownership moves to the queue.
pub(crate) struct Temp(pub PathBuf);
impl Temp {
    pub fn new(ctx: &Ctx) -> ProviderResult<Self> {
        let root = ctx
            .get("_data_dir")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("tmp");
        std::fs::create_dir_all(&root).map_err(io)?;
        let path = root.join(format!("training-{}", ulid::Ulid::new()));
        std::fs::create_dir(&path).map_err(io)?;
        Ok(Self(path))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn io(_: std::io::Error) -> ProviderError {
    ProviderError::Failed("Could not read or write training files.".into())
}

pub fn options(req: &GenerationRequest) -> ProviderResult<TrainingOptions> {
    let opts: TrainingOptions =
        serde_json::from_value(req.params.get("_training").cloned().unwrap_or(Value::Null))
            .map_err(|_| ProviderError::InvalidInput("Missing training settings.".into()))?;
    opts.validate().map_err(ProviderError::InvalidInput)?;
    Ok(opts)
}

/// JSON is a YAML 1.2 subset, so paths and trigger strings are escaped without interpolation.
pub fn config(opts: &TrainingOptions, root: &Path, job: &str) -> Value {
    let model = match opts.family.as_str() {
        "qwen_image" => {
            json!({"name_or_path":"Qwen/Qwen-Image", "arch":"qwen_image", "quantize":true, "quantize_te":true, "low_vram":true})
        }
        "wan21" => {
            json!({"name_or_path":"Wan-AI/Wan2.1-T2V-14B-Diffusers", "arch":"wan21", "quantize":true, "quantize_te":true, "low_vram":true})
        }
        _ => {
            json!({"name_or_path":"black-forest-labs/FLUX.1-dev", "is_flux":true, "quantize":true})
        }
    };
    json!({"job":"extension", "config":{"name":job,"process":[{
        "type":"sd_trainer", "training_folder":root.join("output"), "device":"cuda:0", "trigger_word":opts.trigger_word,
        "network":{"type":"lora","linear":opts.rank,"linear_alpha":opts.rank},
        "save":{"dtype":"float16","save_every":opts.steps,"max_step_saves_to_keep":1,"push_to_hub":false},
        "datasets":[{"folder_path":root.join("images"),"caption_ext":"txt","num_workers":0,"cache_latents_num_workers":1,"cache_latents_to_disk":true,"resolution":[512]}],
        "train":{"batch_size":1,"steps":opts.steps,"gradient_accumulation_steps":1,"train_unet":true,"train_text_encoder":false,
            "gradient_checkpointing":true,"noise_scheduler":"flowmatch","optimizer":"adamw8bit","lr":opts.learning_rate,
            "disable_sampling":true,"dtype":"bf16"},
        "model":model
    }]}})
}

async fn read_progress<R: AsyncRead + Unpin>(mut reader: R, state: Arc<Mutex<State>>) {
    let mut buffer = [0u8; 4096];
    let mut line = Vec::new();
    while let Ok(n) = reader.read(&mut buffer).await {
        if n == 0 {
            break;
        }
        for b in &buffer[..n] {
            if *b == b'\r' || *b == b'\n' {
                if let Some(p) = progress(&String::from_utf8_lossy(&line)) {
                    state.lock().unwrap().progress = Some(p);
                }
                line.clear();
            } else if line.len() < 8192 {
                line.push(*b);
            }
        }
    }
    if let Some(p) = progress(&String::from_utf8_lossy(&line)) {
        state.lock().unwrap().progress = Some(p);
    }
}

async fn execute(
    ctx: Ctx,
    req: GenerationRequest,
    state: Arc<Mutex<State>>,
) -> ProviderResult<(PathBuf, PathBuf)> {
    let opts = options(&req)?;
    let temp = Temp::new(&ctx)?;
    let images = temp.0.join("images");
    tokio::fs::create_dir(&images).await.map_err(io)?;
    for (i, input) in req.inputs.iter().enumerate() {
        let source = input.path.clone();
        let dest = images.join(format!("{i}.png"));
        tokio::task::spawn_blocking(move || {
            let image = image::ImageReader::open(source)?
                .with_guessed_format()?
                .decode()?;
            image.save_with_format(dest, image::ImageFormat::Png)
        })
        .await
        .map_err(|_| ProviderError::Failed("Could not prepare training image.".into()))?
        .map_err(|_| ProviderError::InvalidInput("Could not decode a training image.".into()))?;
        tokio::fs::write(images.join(format!("{i}.txt")), &opts.trigger_word)
            .await
            .map_err(io)?;
    }
    let cfg = temp.0.join("training.yaml");
    tokio::fs::write(
        &cfg,
        serde_json::to_vec_pretty(&config(&opts, &temp.0, &req.job_id))?,
    )
    .await
    .map_err(io)?;
    let configured = PathBuf::from(ctx.require("path")?);
    let mut command = if configured.is_dir() {
        let venv = configured.join(if cfg!(windows) {
            "venv/Scripts/python.exe"
        } else {
            "venv/bin/python"
        });
        let mut cmd = Command::new(if venv.is_file() {
            venv
        } else {
            PathBuf::from("python3")
        });
        cmd.arg(configured.join("run.py")).current_dir(&configured);
        cmd
    } else {
        Command::new(&configured)
    };
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .arg(&cfg)
        .env("PYTHONUNBUFFERED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| {
            ProviderError::Failed(
                "Could not start ai-toolkit. Check its path, Python environment and dependencies."
                    .into(),
            )
        })?;
    #[cfg(unix)]
    let _group = ProcessGroup(child.id());
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let work = async {
        let (status, _, _) = tokio::join!(
            child.wait(),
            read_progress(stdout, state.clone()),
            read_progress(stderr, state)
        );
        status.map_err(io)
    };
    let status = tokio::time::timeout(std::time::Duration::from_secs(24 * 3600), work)
        .await
        .map_err(|_| ProviderError::Failed("Local training exceeded 24 hours.".into()))??;
    if !status.success() {
        return Err(ProviderError::Failed(
            "ai-toolkit failed. Check its local installation, GPU memory and model access.".into(),
        ));
    }
    let output = temp
        .0
        .join("output")
        .join(&req.job_id)
        .join(format!("{}.safetensors", req.job_id));
    let meta = tokio::fs::symlink_metadata(&output).await.map_err(|_| {
        ProviderError::Failed("ai-toolkit returned no final .safetensors file.".into())
    })?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > 4 * 1024 * 1024 * 1024 {
        return Err(ProviderError::Failed(
            "Invalid or oversized LoRA output.".into(),
        ));
    }
    let root = temp.0.clone();
    std::mem::forget(temp);
    Ok((output, root))
}

#[async_trait]
impl Provider for AiToolkit {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "ai-toolkit".into(),
            name: "Local training (ai-toolkit)".into(),
            kind: ProviderKind::Local,
            fields: vec![CredentialField {
                key: "path".into(),
                label: "ai-toolkit folder or executable".into(),
                env: vec!["PICTESA_AI_TOOLKIT_PATH".into()],
                secret: false,
                optional: false,
                default: None,
                placeholder: Some("/path/to/ai-toolkit".into()),
            }],
            signup_url: None,
            docs_url: Some("https://github.com/ostris/ai-toolkit".into()),
            max_concurrency: 1,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        ["flux1", "qwen_image", "wan21"]
            .into_iter()
            .map(|family| {
                let mut m = ModelSpec::new(
                    "ai-toolkit",
                    family,
                    &format!("Train {family} LoRA locally"),
                    MediaKind::Lora,
                    &[Task::Training],
                );
                m.prompt = false;
                m.pricing = Some(Pricing::Local);
                m.meta = json!({"lora_family":family});
                m.inputs.push(InputSlot {
                    min: 3,
                    ..InputSlot::new(InputRole::Reference, MediaKind::Image, 100)
                });
                m
            })
            .collect()
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        let path = PathBuf::from(ctx.require("path")?);
        if path.is_file() || path.join("run.py").is_file() {
            Ok(())
        } else {
            Err(ProviderError::InvalidInput(
                "Set the path to ai-toolkit or an executable wrapper in Settings > Providers."
                    .into(),
            ))
        }
    }
    async fn submit(
        &self,
        ctx: &Ctx,
        _: &ModelSpec,
        req: &GenerationRequest,
    ) -> ProviderResult<Submission> {
        self.check(ctx).await?;
        options(req)?;
        let state = Arc::new(Mutex::new(State::default()));
        let (context, request, status) = (ctx.clone(), req.clone(), state.clone());
        let task = tokio::spawn(async move {
            let result = execute(context, request, status.clone())
                .await
                .map_err(|e| e.to_string());
            status.lock().unwrap().result = Some(result);
        });
        runs()
            .lock()
            .unwrap()
            .insert(req.job_id.clone(), Run { task, state });
        Ok(Submission::Pending(RemoteHandle::with(
            &req.job_id,
            json!({"training":true}),
        )))
    }
    async fn poll(&self, _: &Ctx, h: &RemoteHandle) -> ProviderResult<PollStatus> {
        let mut runs = runs().lock().unwrap();
        let run = runs.get(&h.id).ok_or_else(|| {
            ProviderError::Failed("Local training was interrupted. Retry to start again.".into())
        })?;
        let mut state = run.state.lock().unwrap();
        let Some(result) = state.result.take() else {
            return Ok(PollStatus::Running {
                progress: state.progress,
                message: Some("Training LoRA locally".into()),
            });
        };
        drop(state);
        runs.remove(&h.id);
        Ok(match result {
            Ok((path, cleanup_dir)) => PollStatus::Done(vec![Output::File {
                path,
                mime: MIME.into(),
                cleanup_dir,
            }]),
            Err(message) => PollStatus::Failed(ProviderError::Failed(message)),
        })
    }
    async fn cancel(&self, _: &Ctx, h: &RemoteHandle) -> ProviderResult<()> {
        let run = runs().lock().unwrap().remove(&h.id);
        if let Some(run) = run {
            run.task.abort();
            let _ = run.task.await;
            if let Some(Ok((_, root))) = run.state.lock().unwrap().result.take() {
                let _ = std::fs::remove_dir_all(root);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn toolkit_config_preserves_training_and_escaping() {
        let opts = TrainingOptions {
            trigger_word: "maya".into(),
            steps: 1500,
            rank: 32,
            learning_rate: 0.00005,
            ..Default::default()
        };
        let v = config(&opts, Path::new("/data/with \"quotes\""), "run1");
        assert_eq!(
            v.pointer("/config/process/0/network/linear"),
            Some(&json!(32))
        );
        assert_eq!(
            v.pointer("/config/process/0/train/steps"),
            Some(&json!(1500))
        );
        assert_eq!(
            v.pointer("/config/process/0/train/lr"),
            Some(&json!(0.00005))
        );
        assert_eq!(
            v.pointer("/config/process/0/datasets/0/folder_path"),
            Some(&json!(Path::new("/data/with \"quotes\"").join("images")))
        );
        assert_eq!(
            v.pointer("/config/process/0/save/push_to_hub"),
            Some(&json!(false))
        );
        assert_eq!(
            v.pointer("/config/process/0/model/is_flux"),
            Some(&json!(true))
        );
    }
}

/// Stored ZIP32, streamed from disk. Dataset names are generated, never supplied as paths.
pub async fn archive(req: &GenerationRequest, root: &Path) -> ProviderResult<PathBuf> {
    let inputs = req.inputs.clone();
    let trigger = options(req)?.trigger_word;
    let path = root.join("images.zip");
    let dest = path.clone();
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        use std::io::{Read, Seek, SeekFrom, Write};
        let mut out = std::fs::File::create(dest)?;
        let mut directory = Vec::new();
        let mut count = 0u16;
        for (i, input) in inputs.iter().enumerate() {
            let ext = match input.mime.as_str() {
                "image/jpeg" => "jpg",
                "image/png" => "png",
                "image/webp" => "webp",
                _ => {
                    return Err(std::io::Error::other(
                        "Use PNG, JPEG or WebP training images.",
                    ))
                }
            };
            for (name, mut source) in [
                (
                    format!("image-{i}.{ext}"),
                    Box::new(std::fs::File::open(&input.path)?) as Box<dyn Read>,
                ),
                (
                    format!("image-{i}.txt"),
                    Box::new(std::io::Cursor::new(trigger.as_bytes())) as Box<dyn Read>,
                ),
            ] {
                let start = out.stream_position()? as u32;
                let mut header = Vec::new();
                header.extend_from_slice(&0x04034b50u32.to_le_bytes());
                for n in [20u16, 0x0800, 0, 0, 33] {
                    header.extend_from_slice(&n.to_le_bytes());
                }
                header.extend_from_slice(&[0; 12]);
                header.extend_from_slice(&(name.len() as u16).to_le_bytes());
                header.extend_from_slice(&0u16.to_le_bytes());
                header.extend_from_slice(name.as_bytes());
                out.write_all(&header)?;
                let mut crc = crc32fast::Hasher::new();
                let mut size = 0u32;
                let mut buffer = [0; 65536];
                loop {
                    let n = source.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    crc.update(&buffer[..n]);
                    out.write_all(&buffer[..n])?;
                    size = size
                        .checked_add(n as u32)
                        .ok_or_else(|| std::io::Error::other("Dataset too large"))?;
                }
                let end = out.stream_position()?;
                let crc = crc.finalize();
                out.seek(SeekFrom::Start(start as u64 + 14))?;
                for n in [crc, size, size] {
                    out.write_all(&n.to_le_bytes())?;
                }
                out.seek(SeekFrom::Start(end))?;
                directory.extend_from_slice(&0x02014b50u32.to_le_bytes());
                for n in [20u16, 20, 0x0800, 0, 0, 33] {
                    directory.extend_from_slice(&n.to_le_bytes());
                }
                for n in [crc, size, size] {
                    directory.extend_from_slice(&n.to_le_bytes());
                }
                directory.extend_from_slice(&(name.len() as u16).to_le_bytes());
                directory.extend_from_slice(&[0; 12]);
                directory.extend_from_slice(&start.to_le_bytes());
                directory.extend_from_slice(name.as_bytes());
                count += 1;
            }
        }
        let offset = out.stream_position()? as u32;
        out.write_all(&directory)?;
        out.write_all(&0x06054b50u32.to_le_bytes())?;
        out.write_all(&[0; 4])?;
        out.write_all(&count.to_le_bytes())?;
        out.write_all(&count.to_le_bytes())?;
        out.write_all(&(directory.len() as u32).to_le_bytes())?;
        out.write_all(&offset.to_le_bytes())?;
        out.write_all(&[0; 2])?;
        out.sync_all()
    })
    .await
    .map_err(|_| ProviderError::Failed("Could not prepare training archive.".into()))?
    .map_err(io)?;
    Ok(path)
}

pub fn lora_directory(env: &str) -> CredentialField {
    CredentialField {
        key: "lora_dir".into(),
        label: "Shared LoRA directory (local filesystem)".into(),
        env: vec![env.into()],
        secret: false,
        optional: true,
        default: None,
        placeholder: Some("/path/to/models/loras".into()),
    }
}

#[cfg(unix)]
struct ProcessGroup(Option<u32>);
#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            // The child starts its own group. Cancellation must also stop its data-loader workers.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod zip_tests {
    use super::*;
    #[tokio::test]
    async fn zip_is_readable_with_intact_input_and_trigger_caption() {
        let ctx = Ctx {
            http: reqwest::Client::new(),
            creds: HashMap::new(),
        };
        let temp = Temp::new(&ctx).unwrap();
        let image = temp.0.join("source.png");
        std::fs::write(&image, b"input bytes").unwrap();
        let req = GenerationRequest {
            job_id: "zip-test".into(),
            model: "trainer".into(),
            prompt: String::new(),
            negative_prompt: None,
            params: json!({"_training":{"trigger_word":"maya","family":"flux1"}})
                .as_object()
                .unwrap()
                .clone(),
            inputs: vec![MediaInput {
                role: InputRole::Reference,
                kind: MediaKind::Image,
                mime: "image/png".into(),
                path: image,
                url: None,
                width: None,
                height: None,
            }],
            num_outputs: 1,
        };
        let zip = archive(&req, &temp.0).await.unwrap();
        let status=std::process::Command::new("python3").arg("-c").arg("import sys,zipfile; z=zipfile.ZipFile(sys.argv[1]); assert z.testzip() is None; assert z.namelist()==['image-0.png','image-0.txt']; assert z.read('image-0.png')==b'input bytes'; assert z.read('image-0.txt')==b'maya'").arg(zip).status().unwrap();
        assert!(status.success(), "standard ZIP reader rejected the dataset");
    }
}
