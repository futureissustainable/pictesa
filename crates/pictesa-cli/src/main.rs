//! `pictesa`: one binary for the studio server, the MCP server and the command line.

use std::{io::IsTerminal, net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{anyhow, bail, Context};
use clap::{Parser, Subcommand};
use pictesa_engine::{Config, CreateGeneration, Engine, JobInput, Mode, ModelFilter, LOCAL_USER};
use pictesa_server::{mcp, ServerOptions};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

#[derive(Parser)]
#[command(name = "pictesa", version, about = "Open-source image and video generation studio for people and agents", long_about = None)]
struct Cli {
    /// Where Pictesa keeps its database, files and keys.
    #[arg(long, global = true, env = "PICTESA_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Address of a running Pictesa server (used by `generate` when reachable).
    #[arg(long, global = true, env = "PICTESA_URL", default_value = "http://127.0.0.1:4747")]
    server: String,
    /// Access token for the server, if it requires one.
    #[arg(long, global = true, env = "PICTESA_TOKEN", hide_env_values = true)]
    token: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the studio: web UI, API and MCP endpoint (default command).
    Serve {
        #[arg(long, default_value = "127.0.0.1", env = "PICTESA_HOST")]
        host: String,
        #[arg(long, default_value_t = 4747, env = "PICTESA_PORT")]
        port: u16,
        /// Multi-user mode with accounts (for hosting Pictesa for others).
        #[arg(long, env = "PICTESA_HOSTED")]
        hosted: bool,
        /// Public origin (https://...) so providers can fetch input files by URL.
        #[arg(long, env = "PICTESA_PUBLIC_URL")]
        public_url: Option<String>,
        /// Serve the UI from this folder instead of the embedded build (development).
        #[arg(long)]
        web_dir: Option<PathBuf>,
        /// Open the studio in the default browser.
        #[arg(long)]
        open: bool,
    },
    /// Speak the Model Context Protocol on stdin and stdout (for Claude Code, Codex, Cursor, ...).
    Mcp,
    /// Generate images or video.
    #[command(alias = "gen")]
    Generate {
        /// What to make.
        prompt: Option<String>,
        /// Model id (see `pictesa models`), e.g. fal:fal-ai/flux/dev.
        #[arg(short, long)]
        model: String,
        /// Parameter as key=value (repeatable), e.g. -p aspect_ratio=16:9 -p duration=5.
        #[arg(short, long = "param", value_name = "KEY=VALUE")]
        params: Vec<String>,
        /// Input as role=path or role=asset:<library id> (repeatable). Roles: reference, start_frame, end_frame, video, audio, mask.
        #[arg(short, long = "input", value_name = "ROLE=PATH")]
        inputs: Vec<String>,
        #[arg(short = 'n', long, default_value_t = 1)]
        count: u32,
        #[arg(long)]
        negative: Option<String>,
        #[arg(long)]
        preset: Option<String>,
        /// Copy the results into this folder.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Return right after queueing instead of waiting for the result.
        #[arg(long)]
        no_wait: bool,
        /// Print JSON (for scripts and agents).
        #[arg(long)]
        json: bool,
    },
    /// List models.
    Models {
        #[arg(long)]
        output: Option<String>,
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        provider: Option<String>,
        /// Search text.
        #[arg(short, long)]
        q: Option<String>,
        /// Include models whose provider is not set up.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        featured: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show one model's parameters and inputs.
    Model {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Show providers and whether they are set up.
    Providers {
        #[arg(long)]
        json: bool,
    },
    /// Store or remove provider keys: `pictesa keys set fal api_key` (reads the value from stdin).
    Keys {
        #[command(subcommand)]
        action: KeyAction,
    },
    /// Recent generations.
    Jobs {
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Refresh model catalogs from gateways and local servers.
    Refresh,
    /// Turn a ComfyUI workflow (API format) into a Pictesa model.
    Workflow {
        #[command(subcommand)]
        action: WorkflowAction,
    },
    /// List or run recipes (multi-step pipelines).
    Recipe {
        #[command(subcommand)]
        action: RecipeAction,
    },
    /// Operator tools for hosted billing.
    Billing {
        #[command(subcommand)]
        action: BillingAction,
    },
    /// Check the installation.
    Doctor,
}

#[derive(Subcommand)]
enum WorkflowAction {
    /// `pictesa workflow import my-flow.json --name "My flow"`
    Import {
        file: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Subcommand)]
enum RecipeAction {
    List,
    /// `pictesa recipe run cinema/shot -v scene="a cabin in fog" -v camera=camera/orbit-left`
    Run {
        id: String,
        /// Field value as key=value (repeatable).
        #[arg(short = 'v', long = "value", value_name = "KEY=VALUE")]
        values: Vec<String>,
        /// Input file as key=path (repeatable), for recipes that take photos.
        #[arg(short, long = "input", value_name = "KEY=PATH")]
        inputs: Vec<String>,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum BillingAction {
    /// Give an account an allowance: `pictesa billing grant user@example.com 20 welcome-credit`.
    Grant { email: String, usd: f64, operation: String },
}

#[derive(Subcommand)]
enum KeyAction {
    Set { provider: String, field: String, value: Option<String> },
    Rm { provider: String },
    Check { provider: String },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let quiet = matches!(cli.command, Some(Command::Mcp));
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("PICTESA_LOG").unwrap_or_else(|_| {
            tracing_subscriber::EnvFilter::new(if quiet { "warn" } else { "info,sqlx=warn,tower_http=warn" })
        }))
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
    if let Err(e) = run(cli).await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn data_dir(cli: &Cli) -> PathBuf {
    cli.data_dir.clone().unwrap_or_else(Config::default_data_dir)
}

async fn open_engine(cli: &Cli, mode: Mode, public_url: Option<String>) -> anyhow::Result<Engine> {
    let dir = data_dir(cli);
    let cfg = Config { data_dir: dir.clone(), mode, preset_dirs: vec![dir.join("presets")], public_url };
    Engine::open(cfg).await.with_context(|| format!("opening data directory {}", dir.display()))
}

async fn run(mut cli: Cli) -> anyhow::Result<()> {
    let command = cli.command.take().unwrap_or(Command::Serve { host: "127.0.0.1".into(), port: 4747, hosted: false, public_url: None, web_dir: None, open: true });
    match command {
        Command::Serve { host, port, hosted, public_url, web_dir, open } => {
            let mode = if hosted { Mode::Hosted } else { Mode::Local };
            let engine = open_engine(&cli, mode, public_url.clone()).await?;
            engine.start();
            let ip: std::net::IpAddr = host.parse().map_err(|_| anyhow!("--host must be an IP address"))?;
            if !ip.is_loopback() && mode == Mode::Local && cli.token.is_none() {
                bail!("refusing to listen on {host} without an access token. Set PICTESA_TOKEN (or --token), or use --hosted for accounts.");
            }
            let addr = SocketAddr::new(ip, port);
            if open {
                let url = format!("http://{}:{port}", if ip.is_unspecified() { "127.0.0.1".to_string() } else { host.clone() });
                open_browser(&url);
            }
            eprintln!("Pictesa {} on http://{addr}  (data: {})", env!("CARGO_PKG_VERSION"), engine.data_dir().display());
            let extra: Vec<String> = std::env::var("PICTESA_ALLOWED_HOSTS").unwrap_or_default().split(',').map(String::from).collect();
            pictesa_server::serve(
                engine,
                ServerOptions {
                    addr: Some(addr),
                    token: cli.token.clone(),
                    web_dir,
                    secure_cookies: public_url.as_deref().is_some_and(|u| u.starts_with("https://")),
                    allowed_hosts: ServerOptions::local_hosts(addr, &extra),
                },
            )
            .await
        }
        Command::Mcp => run_mcp(&cli).await,

        Command::Generate { prompt, model, params, inputs, count, negative, preset, out, no_wait, json } => {
            let params = parse_params(&params)?;
            if let Some(client) = Remote::connect(&cli).await {
                return client.generate(prompt, model, params, inputs, count, negative, preset, out, no_wait, json).await;
            }
            generate_local(&cli, prompt, model, params, inputs, count, negative, preset, out, no_wait, json).await
        }
        Command::Models { output, task, provider, q, all, featured, json } => {
            let engine = open_engine(&cli, Mode::Local, None).await?;
            let f = ModelFilter { output, task, provider, q, configured: (!all).then_some(true), featured: featured.then_some(true) };
            let models = engine.models(LOCAL_USER, &f).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&models)?);
            } else if models.is_empty() {
                println!("No models. Set up a provider with `pictesa keys set <provider> api_key`, or pass --all.");
            } else {
                for m in &models {
                    let s = &m.spec;
                    let star = if s.tags.iter().any(|t| t == "featured") { "*" } else { " " };
                    println!("{star} {:<6} {:<58} {}", s.output.as_str(), s.id, s.name);
                }
                println!("\n{} models ({} shown as featured *)", models.len(), models.iter().filter(|m| m.spec.tags.iter().any(|t| t == "featured")).count());
            }
            Ok(())
        }
        Command::Model { id, json } => {
            let engine = open_engine(&cli, Mode::Local, None).await?;
            let spec = engine.model(LOCAL_USER, &id).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&spec)?);
            } else {
                println!("{}  ({})", spec.name, spec.id);
                if let Some(d) = &spec.description {
                    println!("{d}");
                }
                println!("output: {}   max outputs per run: {}", spec.output.as_str(), spec.max_outputs);
                for i in &spec.inputs {
                    let role = serde_json::to_value(i.role)?.as_str().unwrap_or("").to_string();
                    println!("  input  {:<12} {} file(s) of {}{}", role, i.max, i.kind.as_str(), if i.min > 0 { ", required" } else { "" });
                }
                for p in &spec.params {
                    println!("  -p {}=<{}>{}", p.key, serde_json::to_value(&p.kind)?.get("type").and_then(Value::as_str).unwrap_or("value"), p.default.as_ref().map(|d| format!("  (default {d})")).unwrap_or_default());
                }
            }
            Ok(())
        }
        Command::Providers { json } => {
            let engine = open_engine(&cli, Mode::Local, None).await?;
            let list = engine.providers(LOCAL_USER).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else {
                for p in &list {
                    let env: Vec<String> = p.fields.iter().flat_map(|f| f.env.clone()).collect();
                    println!("{} {:<14} {:>5} models   env: {}", if p.configured { "ok " } else { " - " }, p.id, p.model_count, env.join(", "));
                }
            }
            Ok(())
        }
        Command::Keys { action } => {
            let engine = open_engine(&cli, Mode::Local, None).await?;
            match action {
                KeyAction::Set { provider, field, value } => {
                    let value = match value {
                        Some(v) => v,
                        None => {
                            if std::io::stdin().is_terminal() {
                                eprint!("{provider} {field}: ");
                            }
                            let mut line = String::new();
                            std::io::stdin().read_line(&mut line)?;
                            line.trim().to_string()
                        }
                    };
                    let mut values = Map::new();
                    values.insert(field, json!(value));
                    let state = engine.set_credentials(LOCAL_USER, &provider, &values).await?;
                    println!("{} is {}", state.name, if state.configured { "set up" } else { "missing fields" });
                    match engine.check_provider(LOCAL_USER, &provider).await? {
                        Ok(()) => println!("check: ok"),
                        Err(e) => println!("check failed: {e}"),
                    }
                }
                KeyAction::Rm { provider } => {
                    engine.clear_credentials(LOCAL_USER, &provider).await?;
                    println!("removed stored values for {provider}");
                }
                KeyAction::Check { provider } => match engine.check_provider(LOCAL_USER, &provider).await? {
                    Ok(()) => println!("ok"),
                    Err(e) => bail!("{e}"),
                },
            }
            Ok(())
        }
        Command::Jobs { limit, json } => {
            let engine = open_engine(&cli, Mode::Local, None).await?;
            let (jobs, _) = engine.jobs(LOCAL_USER, &pictesa_engine::JobFilter { limit: Some(limit), ..Default::default() }).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&jobs)?);
            } else {
                for j in &jobs {
                    println!("{}  {:<9} {:<40} {}", j.id, j.status.as_str(), j.model, j.prompt.chars().take(60).collect::<String>());
                }
            }
            Ok(())
        }
        // Both go through a running server when there is one, so its model list changes at once.
        Command::Refresh => {
            let n = match Remote::connect(&cli).await {
                Some(remote) => remote.json(remote.req(reqwest::Method::POST, "/api/models/refresh")).await?["models"].as_u64().unwrap_or(0) as usize,
                None => open_engine(&cli, Mode::Local, None).await?.refresh_models(LOCAL_USER).await,
            };
            println!("{n} models discovered");
            Ok(())
        }
        Command::Workflow { action: WorkflowAction::Import { file, name } } => {
            let text = tokio::fs::read_to_string(&file).await.with_context(|| format!("reading {}", file.display()))?;
            let workflow: Value = serde_json::from_str(&text).with_context(|| format!("{} is not JSON", file.display()))?;
            let name = name.unwrap_or_else(|| file.file_stem().map(|s| s.to_string_lossy().replace(['_', '-'], " ")).unwrap_or_else(|| "Workflow".into()));
            let (model, notes) = match Remote::connect(&cli).await {
                Some(remote) => {
                    let v = remote.json(remote.req(reqwest::Method::POST, "/api/workflows/import").json(&json!({ "name": name, "workflow": workflow }))).await?;
                    let notes = v["notes"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
                    (v["model"].as_str().unwrap_or_default().to_string(), notes)
                }
                None => {
                    let done = open_engine(&cli, Mode::Local, None).await?.import_comfy_workflow(LOCAL_USER, &name, workflow).await?;
                    (done.model, done.notes)
                }
            };
            println!("Added {model} ({name})");
            for n in &notes {
                println!("  note: {n}");
            }
            println!("Try it: pictesa model {model}");
            Ok(())
        }
        Command::Recipe { action } => {
            let engine = open_engine(&cli, Mode::Local, None).await?;
            match action {
                RecipeAction::List => {
                    for r in engine.recipes(LOCAL_USER).await? {
                        println!("{} {:<24} {}", if r.runnable { "ok" } else { " -" }, r.recipe.id, r.recipe.description);
                    }
                }
                RecipeAction::Run { id, values, inputs, out } => {
                    engine.start_scoped();
                    let mut req = pictesa_engine::recipes::RunRecipe { values: parse_params(&values)?, ..Default::default() };
                    for raw in &inputs {
                        let (key, path) = raw.split_once('=').ok_or_else(|| anyhow!("input {raw} must look like key=path"))?;
                        let bytes = tokio::fs::read(path).await.with_context(|| format!("reading {path}"))?;
                        let asset = engine.upload(LOCAL_USER, &bytes, std::path::Path::new(path).file_name().and_then(|n| n.to_str()), None).await?;
                        req.inputs.entry(key.to_string()).or_default().push(asset.id);
                    }
                    let mut run = engine.run_recipe(LOCAL_USER, &id, req).await?;
                    let mut shown = 0;
                    while run.status == "running" {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        run = engine.recipe_run(LOCAL_USER, &run.id).await?;
                        if run.jobs.len() > shown {
                            shown = run.jobs.len();
                            eprintln!("step {}/{} running", shown, run.steps);
                        }
                    }
                    if run.status != "succeeded" {
                        bail!("recipe {}: {}", run.status, run.error.unwrap_or_default());
                    }
                    let mut jobs = Vec::new();
                    for j in &run.jobs {
                        jobs.push(engine.job(LOCAL_USER, j).await?);
                    }
                    report(&engine, jobs, out, false).await?;
                }
            }
            Ok(())
        }
        Command::Billing { action: BillingAction::Grant { email, usd, operation } } => {
            let engine = open_engine(&cli, Mode::Hosted, None).await?;
            engine.grant(&email, usd, &operation).await?;
            println!("granted ${usd:.2} to {email} ({operation})");
            Ok(())
        }
        Command::Doctor => {
            let engine = open_engine(&cli, Mode::Local, None).await?;
            println!("pictesa {}", env!("CARGO_PKG_VERSION"));
            println!("data dir: {}", engine.data_dir().display());
            println!("ffmpeg:   {}", if engine.has_ffmpeg() { "found" } else { "missing (video thumbnails and durations disabled)" });
            let providers = engine.providers(LOCAL_USER).await?;
            let ready: Vec<&str> = providers.iter().filter(|p| p.configured).map(|p| p.id.as_str()).collect();
            println!("providers set up: {}", if ready.is_empty() { "none".to_string() } else { ready.join(", ") });
            println!("server:   {}", if Remote::connect(&cli).await.is_some() { format!("running at {}", cli.server) } else { "not running".into() });
            Ok(())
        }
    }
}

fn parse_params(raw: &[String]) -> anyhow::Result<Map<String, Value>> {
    let mut out = Map::new();
    for kv in raw {
        let (k, v) = kv.split_once('=').ok_or_else(|| anyhow!("parameter {kv} must look like key=value"))?;
        // Numbers, booleans and JSON pass through typed; everything else is a string.
        let value = serde_json::from_str::<Value>(v).unwrap_or_else(|_| json!(v));
        out.insert(k.trim().to_string(), value);
    }
    Ok(out)
}

fn parse_input(raw: &str) -> anyhow::Result<(pictesa_core::InputRole, String)> {
    let (role, path) = raw.split_once('=').ok_or_else(|| anyhow!("input {raw} must look like role=path"))?;
    let role: pictesa_core::InputRole = serde_json::from_value(json!(role.trim())).map_err(|_| anyhow!("unknown input role {role}; use reference, start_frame, end_frame, video, audio or mask"))?;
    Ok((role, path.trim().to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn generate_local(
    cli: &Cli,
    prompt: Option<String>,
    model: String,
    params: Map<String, Value>,
    inputs: Vec<String>,
    count: u32,
    negative: Option<String>,
    preset: Option<String>,
    out: Option<PathBuf>,
    no_wait: bool,
    json_out: bool,
) -> anyhow::Result<()> {
    let engine = open_engine(cli, Mode::Local, None).await?;
    engine.start_scoped();
    let mut job_inputs = Vec::new();
    for raw in &inputs {
        let (role, path) = parse_input(raw)?;
        if let Some(id) = path.strip_prefix("asset:") {
            job_inputs.push(JobInput { asset_id: id.to_string(), role });
            continue;
        }
        let bytes = tokio::fs::read(&path).await.with_context(|| format!("reading {path}"))?;
        let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned());
        let asset = engine.upload(LOCAL_USER, &bytes, name.as_deref(), None).await?;
        job_inputs.push(JobInput { asset_id: asset.id, role });
    }
    let created = engine
        .create_generation(LOCAL_USER, CreateGeneration { model, prompt: prompt.unwrap_or_default(), negative_prompt: negative, params, inputs: job_inputs, count: Some(count), preset, compare: Vec::new(), character: None })
        .await?;
    if no_wait {
        eprintln!("No server is running, so the job would stop when this command exits. Waiting instead.");
    }
    let mut done = Vec::new();
    for j in &created.jobs {
        if !json_out {
            eprintln!("queued {} on {}", j.id, j.model);
        }
        let job = engine.wait_job(LOCAL_USER, &j.id, Duration::from_secs(3 * 3600)).await?;
        done.push(job);
    }
    report(&engine, done, out, json_out).await
}

async fn report(engine: &Engine, jobs: Vec<pictesa_engine::Job>, out: Option<PathBuf>, json_out: bool) -> anyhow::Result<()> {
    let mut results = Vec::new();
    let mut failed = false;
    for j in &jobs {
        let mut files = Vec::new();
        for a in &j.outputs {
            let (path, _, name) = engine.asset_file(Some(LOCAL_USER), &a.id).await?;
            let final_path = match &out {
                Some(dir) => {
                    tokio::fs::create_dir_all(dir).await?;
                    let dest = dir.join(name);
                    tokio::fs::copy(&path, &dest).await?;
                    dest
                }
                None => path,
            };
            files.push(json!({ "asset_id": a.id, "path": final_path.display().to_string(), "kind": a.kind, "width": a.width, "height": a.height }));
        }
        if j.status != pictesa_engine::JobStatus::Succeeded {
            failed = true;
        }
        results.push(json!({ "id": j.id, "status": j.status, "error": j.error, "cost_usd": j.cost_usd, "files": files }));
    }
    if json_out {
        println!("{}", serde_json::to_string_pretty(&json!({ "jobs": results }))?);
    } else {
        for r in &results {
            match r["status"].as_str() {
                Some("succeeded") => {
                    for f in r["files"].as_array().into_iter().flatten() {
                        println!("{}", f["path"].as_str().unwrap_or_default());
                    }
                }
                other => eprintln!("{} {}: {}", r["id"].as_str().unwrap_or(""), other.unwrap_or("?"), r["error"]["message"].as_str().unwrap_or("")),
            }
        }
    }
    if failed {
        bail!("some generations did not succeed");
    }
    Ok(())
}

/// Client for a running Pictesa server, so CLI jobs show up live in the UI.
struct Remote {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl Remote {
    async fn connect(cli: &Cli) -> Option<Self> {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(30)).build().ok()?;
        let base = cli.server.trim_end_matches('/').to_string();
        let ok = http.get(format!("{base}/api/health")).timeout(Duration::from_secs(3)).send().await.ok()?.status().is_success();
        ok.then(|| Self { http, base, token: cli.token.clone() })
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let r = self.http.request(method, format!("{}{path}", self.base));
        match &self.token {
            Some(t) => r.bearer_auth(t),
            None => r,
        }
    }

    async fn json(&self, r: reqwest::RequestBuilder) -> anyhow::Result<Value> {
        let resp = r.send().await?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            bail!("{}", body.get("error").and_then(Value::as_str).unwrap_or(&format!("HTTP {status}")));
        }
        Ok(body)
    }

    #[allow(clippy::too_many_arguments)]
    async fn generate(
        &self,
        prompt: Option<String>,
        model: String,
        params: Map<String, Value>,
        inputs: Vec<String>,
        count: u32,
        negative: Option<String>,
        preset: Option<String>,
        out: Option<PathBuf>,
        no_wait: bool,
        json_out: bool,
    ) -> anyhow::Result<()> {
        let mut job_inputs = Vec::new();
        for raw in &inputs {
            let (role, path) = parse_input(raw)?;
            if let Some(id) = path.strip_prefix("asset:") {
                job_inputs.push(json!({ "asset_id": id, "role": role }));
                continue;
            }
            let bytes = tokio::fs::read(&path).await.with_context(|| format!("reading {path}"))?;
            let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "input".into());
            let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(bytes).file_name(name));
            let v = self.json(self.req(reqwest::Method::POST, "/api/assets").multipart(form)).await?;
            let id = v.pointer("/assets/0/id").and_then(Value::as_str).ok_or_else(|| anyhow!("upload failed"))?;
            job_inputs.push(json!({ "asset_id": id, "role": role }));
        }
        let body = json!({ "model": model, "prompt": prompt.unwrap_or_default(), "negative_prompt": negative, "params": params, "inputs": job_inputs, "count": count, "preset": preset });
        let created = self.json(self.req(reqwest::Method::POST, "/api/generations").json(&body)).await?;
        let ids: Vec<String> = created["jobs"].as_array().into_iter().flatten().filter_map(|j| j["id"].as_str().map(String::from)).collect();
        if no_wait {
            println!("{}", if json_out { serde_json::to_string_pretty(&created)? } else { ids.join("\n") });
            return Ok(());
        }
        let mut results = Vec::new();
        let mut failed = false;
        for id in ids {
            if !json_out {
                eprintln!("queued {id}");
            }
            let job = loop {
                let j = self.json(self.req(reqwest::Method::GET, &format!("/api/generations/{id}"))).await?;
                if matches!(j["status"].as_str(), Some("succeeded" | "failed" | "canceled")) {
                    break j;
                }
                if !json_out && std::io::stderr().is_terminal() {
                    let progress = j["progress"].as_f64().map(|p| format!(" {:.0}%", p * 100.0)).unwrap_or_default();
                    eprint!("\r{} {}{}            ", id, j["message"].as_str().unwrap_or("working"), progress);
                }
                tokio::time::sleep(Duration::from_millis(1500)).await;
            };
            if !json_out && std::io::stderr().is_terminal() {
                eprintln!();
            }
            let mut files = Vec::new();
            for a in job["outputs"].as_array().into_iter().flatten() {
                let aid = a["id"].as_str().unwrap_or_default();
                let path = match &out {
                    Some(dir) => {
                        tokio::fs::create_dir_all(dir).await?;
                        let ext = a["mime"].as_str().map(pictesa_engine::files::ext_for).unwrap_or("bin");
                        let dest = dir.join(format!("pictesa-{}.{ext}", aid.to_lowercase()));
                        let bytes = self.req(reqwest::Method::GET, &format!("/files/{aid}")).send().await?.bytes().await?;
                        let mut f = tokio::fs::File::create(&dest).await?;
                        f.write_all(&bytes).await?;
                        dest.display().to_string()
                    }
                    None => format!("{}/files/{aid}", self.base),
                };
                files.push(json!({ "asset_id": aid, "path": path, "kind": a["kind"] }));
            }
            if job["status"] != "succeeded" {
                failed = true;
            }
            results.push(json!({ "id": job["id"], "status": job["status"], "error": job["error"], "cost_usd": job["cost_usd"], "files": files }));
        }
        if json_out {
            println!("{}", serde_json::to_string_pretty(&json!({ "jobs": results }))?);
        } else {
            for r in &results {
                if r["status"] == "succeeded" {
                    for f in r["files"].as_array().into_iter().flatten() {
                        println!("{}", f["path"].as_str().unwrap_or_default());
                    }
                } else {
                    eprintln!("{} {}: {}", r["id"].as_str().unwrap_or(""), r["status"].as_str().unwrap_or("?"), r["error"]["message"].as_str().unwrap_or(""));
                }
            }
        }
        if failed {
            bail!("some generations did not succeed");
        }
        Ok(())
    }
}

/// MCP over stdio: newline-delimited JSON-RPC. Logs go to stderr so stdout stays clean.
async fn run_mcp(cli: &Cli) -> anyhow::Result<()> {
    let engine = open_engine(cli, Mode::Local, None).await?;
    // Scoped: this process runs only the jobs its agent creates and never adopts the studio's work.
    engine.start_scoped();
    let ctx = mcp::McpContext { engine, user_id: LOCAL_USER.to_string(), local_files: true };
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                let reply = json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("Parse error: {e}") } });
                stdout.write_all(format!("{reply}\n").as_bytes()).await?;
                stdout.flush().await?;
                continue;
            }
        };
        let replies: Vec<Value> = match msg {
            Value::Array(batch) => {
                let mut out = Vec::new();
                for m in batch {
                    if let Some(r) = mcp::handle(&ctx, m).await {
                        out.push(r);
                    }
                }
                if out.is_empty() { Vec::new() } else { vec![Value::Array(out)] }
            }
            m => mcp::handle(&ctx, m).await.into_iter().collect(),
        };
        for r in replies {
            stdout.write_all(format!("{r}\n").as_bytes()).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

fn open_browser(url: &str) {
    let (cmd, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(windows) {
        ("cmd", vec!["/C", "start", "", url])
    } else {
        ("xdg-open", vec![url])
    };
    let _ = std::process::Command::new(cmd).args(args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
}
