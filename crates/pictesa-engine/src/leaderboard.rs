//! Arena leaderboards: LMArena's public, daily dataset of human-preference ratings for image and
//! video models (huggingface.co/datasets/lmarena-ai/leaderboard-dataset). Ratings are matched to
//! catalog models by name, so the studio and agents can sort and label models by how people rate them.

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
    time::Duration,
};

use pictesa_core::{now_ms, ModelSpec, Task};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Engine;

/// Dataset configs Pictesa reads, with the task each one rates.
pub const ARENAS: [(&str, Task); 4] = [
    ("text_to_image", Task::TextToImage),
    ("image_edit", Task::ImageEdit),
    ("text_to_video", Task::TextToVideo),
    ("image_to_video", Task::ImageToVideo),
];

const ROWS: &str = "https://datasets-server.huggingface.co/rows?dataset=lmarena-ai/leaderboard-dataset";
const DAY_MS: i64 = 86_400_000;
/// The rows of the `settings` table that hold the downloaded arenas.
const SETTINGS_USER: &str = "system";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArenaEntry {
    pub name: String,
    pub rating: f64,
    pub rank: u32,
    pub votes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Arena {
    pub key: String,
    /// The leaderboard's date (YYYY-MM-DD).
    pub published: String,
    pub fetched_at: i64,
    pub entries: Vec<ArenaEntry>,
}

/// One model's standing in one arena.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ArenaScore {
    pub arena: String,
    /// The model's name on the leaderboard.
    pub name: String,
    pub rank: u32,
    /// Models rated in that arena.
    pub of: u32,
    pub rating: f64,
    pub votes: u64,
    pub published: String,
}

/// Downloads one arena's overall ranking (the dataset also holds sub-categories).
pub async fn fetch(http: &reqwest::Client, key: &str) -> anyhow::Result<Arena> {
    let mut entries = Vec::new();
    let mut published = String::new();
    let mut offset = 0usize;
    loop {
        let url = format!("{ROWS}&config={key}&split=latest&offset={offset}&length=100");
        let page: Value = http.get(&url).timeout(Duration::from_secs(30)).send().await?.error_for_status()?.json().await?;
        let rows = page.get("rows").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
        for row in rows.iter().filter_map(|r| r.get("row")) {
            if row.get("category").and_then(Value::as_str) != Some("overall") {
                continue;
            }
            let (Some(name), Some(rating)) = (row.get("model_name").and_then(Value::as_str), row.get("rating").and_then(Value::as_f64)) else { continue };
            if published.is_empty() {
                published = row.get("leaderboard_publish_date").and_then(Value::as_str).unwrap_or_default().to_string();
            }
            entries.push(ArenaEntry {
                name: name.to_string(),
                rating,
                rank: row.get("rank").and_then(Value::as_u64).unwrap_or(0) as u32,
                votes: row.get("vote_count").and_then(Value::as_u64).unwrap_or(0),
            });
        }
        offset += rows.len();
        let total = page.get("num_rows_total").and_then(Value::as_u64).unwrap_or(0) as usize;
        if rows.is_empty() || offset >= total {
            break;
        }
    }
    anyhow::ensure!(!entries.is_empty(), "the {key} leaderboard came back empty");
    Ok(Arena { key: key.to_string(), published, fetched_at: now_ms(), entries })
}

/// Words that name a company, a region, a task or an output format rather than a model.
const NOISE: &[&str] = &[
    "openai", "google", "vertex", "vertexai", "bytedance", "byteplus", "dreamina", "alibaba", "bfl", "blackforestlabs", "minimax", "xai", "luma",
    "lumaai", "ai", "fal", "meta", "microsoft", "tencent", "runway", "runwayml", "lightricks", "stability", "stabilityai", "shengshu", "nvidia",
    "us", "eu", "global", "text", "to", "image", "images", "img", "video", "videos", "edit", "editing", "i2v", "t2v", "t2i", "i2i", "generate",
    "generation", "preview", "latest", "quality", "medium", "high", "low", "fidelity", "audio", "web", "search", "v",
];

/// Service tiers: a catalog id may add one to a leaderboard name and still be the same model
/// ("kling-video/v2.5-turbo/pro" is "kling-2.5-turbo").
const TIERS: &[&str] = &["pro", "standard", "std", "hd", "prime"];

/// The words that identify a model: lowercase, versions without a trailing ".0", no company,
/// task, format, resolution or date words. "fal-ai/kling-video/v2.6/pro/text-to-video" and
/// "kling-2.6-pro" both become {kling, 2.6, pro}.
fn words(name: &str) -> BTreeSet<String> {
    let lower = strip_dates(&name.to_lowercase());
    let mut out = BTreeSet::new();
    for chunk in lower.split(|c: char| !c.is_ascii_alphanumeric() && c != '.') {
        let chunk = chunk.trim_matches('.');
        if chunk.is_empty() || is_format(chunk) || NOISE.contains(&chunk) {
            continue;
        }
        // Split letter runs from number runs: "wan3.0" -> "wan", "3.0"; "gen4.5" -> "gen", "4.5".
        let mut token = String::new();
        let mut digits = false;
        for c in chunk.chars() {
            let d = c.is_ascii_digit() || (c == '.' && digits);
            if !token.is_empty() && d != digits {
                push_word(&mut out, &token);
                token.clear();
            }
            digits = d;
            token.push(c);
        }
        push_word(&mut out, &token);
    }
    // Google's names say "gemini" where catalogs say "nano-banana".
    if out.contains("nano") && out.contains("banana") {
        out.remove("gemini");
    }
    out
}

/// Removes YYYY-MM-DD dates, which would otherwise leave month and day numbers behind.
fn strip_dates(s: &str) -> String {
    let b = s.as_bytes();
    let is_date = |i: usize| b.get(i..i + 10).is_some_and(|w| w.iter().enumerate().all(|(j, c)| if j == 4 || j == 7 { *c == b'-' } else { c.is_ascii_digit() }));
    let mut out = String::with_capacity(s.len());
    let mut skip_to = 0;
    for (i, c) in s.char_indices() {
        if i < skip_to {
            continue;
        }
        if (i == 0 || !b[i - 1].is_ascii_digit()) && is_date(i) {
            out.push(' ');
            skip_to = i + 10;
        } else {
            out.push(c);
        }
    }
    out
}

/// Resolutions (720p, 2k), dates and builds (20260801, 260628, 2026, 001) and number formats (fp8, bf16).
fn is_format(chunk: &str) -> bool {
    let digits = chunk.trim_end_matches(['p', 'k']);
    let resolution = digits.len() < chunk.len() && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit());
    let numeric = chunk.chars().all(|c| c.is_ascii_digit());
    let number_format = ["fp", "bf", "int"].iter().any(|p| chunk.strip_prefix(p).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())));
    resolution || number_format || (numeric && (chunk.len() >= 6 || (chunk.len() == 4 && chunk.starts_with("20")) || (chunk.len() >= 3 && chunk.starts_with('0'))))
}

fn push_word(out: &mut BTreeSet<String>, token: &str) {
    let mut word = token.trim_matches('.');
    // "HunyuanImage" names the same model as "hunyuan-image".
    for suffix in ["image", "video"] {
        word = word.strip_suffix(suffix).filter(|w| !w.is_empty()).unwrap_or(word);
    }
    if word.is_empty() || NOISE.contains(&word) {
        return;
    }
    let mut word = word.to_string();
    while word.contains('.') && word.ends_with(".0") {
        word.truncate(word.len() - 2);
    }
    out.insert(word);
}

/// The word sets a leaderboard entry goes by: its name, and Google's alias in parentheses
/// ("gemini-3-pro-image-2k (nano-banana-pro)"). Other parentheses hold variants of the name.
fn entry_names(name: &str) -> Vec<BTreeSet<String>> {
    let mut main = String::new();
    let mut aliases = Vec::new();
    let mut rest = name;
    while let Some(open) = rest.find('(') {
        main.push_str(&rest[..open]);
        let Some(close) = rest[open..].find(')') else { break };
        let inner = &rest[open + 1..open + close];
        if inner.to_lowercase().contains("nano-banana") {
            aliases.push(words(inner));
        } else {
            main.push(' ');
            main.push_str(inner);
        }
        rest = &rest[open + close + 1..];
    }
    main.push_str(rest);
    let mut names = vec![words(&main)];
    names.extend(aliases);
    names.retain(|w| !w.is_empty());
    names
}

/// Leaderboards ready for lookups: every entry filed under the words of its names.
#[derive(Default)]
pub struct Boards {
    boards: Vec<Board>,
    /// When the oldest arena was downloaded; 0 while one is missing.
    pub fetched_at: i64,
}

struct Board {
    key: String,
    task: Task,
    published: String,
    size: u32,
    by_words: HashMap<BTreeSet<String>, ArenaEntry>,
}

impl Boards {
    pub fn new(arenas: &[Arena]) -> Self {
        let boards = arenas
            .iter()
            .filter_map(|a| {
                let task = ARENAS.iter().find(|(key, _)| *key == a.key)?.1;
                let mut by_words: HashMap<BTreeSet<String>, ArenaEntry> = HashMap::new();
                for e in &a.entries {
                    // One model rated at two resolutions or settings: the most-voted entry speaks for it.
                    for names in entry_names(&e.name) {
                        let slot = by_words.entry(names).or_insert_with(|| e.clone());
                        if e.votes > slot.votes {
                            *slot = e.clone();
                        }
                    }
                }
                Some(Board { key: a.key.clone(), task, published: a.published.clone(), size: a.entries.len() as u32, by_words })
            })
            .collect();
        let complete = ARENAS.iter().all(|(key, _)| arenas.iter().any(|a| a.key == *key));
        let fetched_at = if complete { arenas.iter().map(|a| a.fetched_at).min().unwrap_or(0) } else { 0 };
        Self { boards, fetched_at }
    }

    /// The model's standing in each arena that rates one of its tasks.
    pub fn scores(&self, model: &ModelSpec) -> Vec<ArenaScore> {
        let boards: Vec<&Board> = self.boards.iter().filter(|b| model.tasks.contains(&b.task)).collect();
        if boards.is_empty() {
            return Vec::new();
        }
        let id = model.remote_id();
        let mut names = vec![words(id)];
        // Some catalogs put a maker in front that the leaderboard leaves out ("wan/happyhorse-1.0-t2v").
        if let Some(rest) = id.split_once('/').map(|(_, rest)| words(rest)).filter(|w| *w != names[0] && w.iter().any(|w| w.chars().any(|c| c.is_ascii_alphabetic()))) {
            names.push(rest);
        }
        let candidates: Vec<BTreeSet<String>> = names.iter().flat_map(without_tiers).collect();
        boards
            .iter()
            .filter_map(|b| {
                let e = candidates.iter().find_map(|c| b.by_words.get(c))?;
                Some(ArenaScore { arena: b.key.clone(), name: e.name.clone(), rank: e.rank, of: b.size, rating: e.rating, votes: e.votes, published: b.published.clone() })
            })
            .collect()
    }
}

/// The words themselves first, then without the tier words they contain.
fn without_tiers(ids: &BTreeSet<String>) -> Vec<BTreeSet<String>> {
    let tiers: Vec<&String> = ids.iter().filter(|w| TIERS.contains(&w.as_str())).take(4).collect();
    let mut masks: Vec<u32> = (0..1u32 << tiers.len()).collect();
    masks.sort_by_key(|m| m.count_ones());
    masks.iter().map(|m| ids.iter().filter(|w| !tiers.iter().enumerate().any(|(i, t)| m & (1 << i) != 0 && t == w)).cloned().collect()).collect()
}

impl Engine {
    /// Loads the arenas downloaded earlier, so scores show without waiting for the network.
    pub(crate) async fn load_arenas(&self) {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE user_id = ? AND key LIKE 'arena:%'")
            .bind(SETTINGS_USER)
            .fetch_all(&self.inner.db)
            .await
            .unwrap_or_default();
        let arenas: Vec<Arena> = rows.iter().filter_map(|(v,)| serde_json::from_str(v).ok()).collect();
        *self.inner.arenas.write().unwrap() = Arc::new(Boards::new(&arenas));
    }

    /// Downloads every arena again. An arena that fails keeps its last copy.
    pub async fn refresh_arenas(&self) -> anyhow::Result<()> {
        let mut failed = None;
        for (key, _) in ARENAS {
            match fetch(&self.inner.http, key).await {
                Ok(arena) => crate::store::put_setting(&self.inner.db, SETTINGS_USER, &format!("arena:{key}"), &serde_json::to_string(&arena)?).await?,
                Err(e) => failed = Some(e.context(format!("the {key} leaderboard"))),
            }
        }
        self.load_arenas().await;
        failed.map_or(Ok(()), Err)
    }

    /// Keeps the arenas at most a day old while the server runs. `PICTESA_LEADERBOARDS=off` turns it off.
    pub fn start_leaderboards(&self) {
        if std::env::var("PICTESA_LEADERBOARDS").is_ok_and(|v| matches!(v.as_str(), "0" | "off" | "false")) {
            return;
        }
        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                let fetched_at = engine.inner.arenas.read().unwrap().fetched_at;
                if now_ms() - fetched_at > DAY_MS {
                    if let Err(e) = engine.refresh_arenas().await {
                        tracing::info!("leaderboards not updated: {e:#}");
                    }
                }
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        });
    }

    pub(crate) fn arenas(&self) -> Arc<Boards> {
        self.inner.arenas.read().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pictesa_core::MediaKind;

    fn arena(key: &str, names: &[&str]) -> Arena {
        let entries = names.iter().enumerate().map(|(i, n)| ArenaEntry { name: n.to_string(), rating: 1500.0 - i as f64 * 10.0, rank: i as u32 + 1, votes: 1000 }).collect();
        Arena { key: key.into(), published: "2026-10-07".into(), fetched_at: 0, entries }
    }

    fn model(id: &str, task: Task) -> ModelSpec {
        let (provider, rest) = id.split_once(':').unwrap();
        let output = if matches!(task, Task::TextToVideo | Task::ImageToVideo) { MediaKind::Video } else { MediaKind::Image };
        ModelSpec::new(provider, rest, rest, output, &[task])
    }

    /// Leaderboard names as LMArena published them (22.09 to 20.10.2026), against catalog ids.
    #[test]
    fn leaderboard_names_find_their_catalog_models() {
        let boards = Boards::new(&[
            arena("text_to_video", &["veo-3.1-audio", "veo-3.1-fast-audio", "kling-2.6-pro", "kling-2.5-turbo-1080p", "dreamina-seedance-2.5-720p", "sora-2-pro", "sora-2", "hailuo-2.3", "hailuo-02-standard", "flux-3-video", "minimax-h3", "wan3.0", "wan-v2.2-a14b", "happyhorse-1.0"]),
            arena("text_to_image", &["gpt-image-2.5-sunburst", "gemini-3-pro-image-2k (nano-banana-pro)", "gemini-nano-banana-2.1", "seedream-5.0-pro", "gemini-3.1-flash-image (nano-banana-2) [web-search]", "qwen-image-2.0-pro-2026-06-22", "gpt-image-2 (medium)", "qwen-image"]),
            arena("image_edit", &["GPT-Image-2.5 Sunburst", "Gemini 3 Pro Image (nano-banana-pro, 2K)", "FLUX.2 [max]", "HunyuanImage 3.0 Instruct", "Qwen-Image-Edit (2511)", "ChatGPT Image Latest (High Fidelity, 20251216)"]),
        ]);
        let rank = |id: &str, task: Task| boards.scores(&model(id, task)).first().map(|s| s.rank);
        use Task::*;
        assert_eq!(rank("fal:fal-ai/veo3.1", TextToVideo), Some(1));
        assert_eq!(rank("comfy_router:veo/veo-3.1-generate-001", TextToVideo), Some(1));
        assert_eq!(rank("comfy_router:veo/veo-3.1-fast-generate-001", TextToVideo), Some(2), "fast is its own model");
        assert_eq!(rank("fal:fal-ai/kling-video/v2.6/pro/text-to-video", TextToVideo), Some(3));
        assert_eq!(rank("fal:fal-ai/kling-video/v2.5-turbo/pro/text-to-video", TextToVideo), Some(4), "a tier word may be added");
        assert_eq!(rank("fal:bytedance/seedance-2.5/us/text-to-video", TextToVideo), Some(5));
        assert_eq!(rank("openai:sora-2-pro", TextToVideo), Some(6), "the exact name wins over a tier");
        assert_eq!(rank("openai:sora-2", TextToVideo), Some(7));
        assert_eq!(rank("fal:fal-ai/minimax/hailuo-2.3/standard/text-to-video", TextToVideo), Some(8));
        assert_eq!(rank("fal:fal-ai/minimax/hailuo-02/standard/text-to-video", TextToVideo), Some(9));
        assert_eq!(rank("fal:blackforestlabs/flux-3/text-to-video", TextToVideo), Some(10));
        assert_eq!(rank("fal:minimax/h3/text-to-video", TextToVideo), Some(11));
        assert_eq!(rank("fal:alibaba/wan-3.0-prime/text-to-video", TextToVideo), Some(12));
        assert_eq!(rank("fal:fal-ai/wan/v2.2-a14b/text-to-video", TextToVideo), Some(13));
        assert_eq!(rank("comfy_router:wan/happyhorse-1.0-t2v", TextToVideo), Some(14), "a maker in front of the name");
        assert_eq!(rank("openai:gpt-image-2.5-sunburst", TextToImage), Some(1));
        assert_eq!(rank("fal:openai/gpt-image-2.5/sunburst/text-to-image", TextToImage), Some(1));
        assert_eq!(rank("google:gemini-3-pro-image-preview", TextToImage), Some(2));
        assert_eq!(rank("fal:fal-ai/nano-banana-pro", TextToImage), Some(2), "Google's alias in parentheses");
        assert_eq!(rank("comfy_router:vertexai/gemini-nano-banana-2.1", TextToImage), Some(3));
        assert_eq!(rank("fal:bytedance/seedream/v5/pro/text-to-image", TextToImage), Some(4), "v5 and 5.0 are one version");
        assert_eq!(rank("google:gemini-3.1-flash-image", TextToImage), Some(5), "web search is a mode, not a model");
        assert_eq!(rank("fal:fal-ai/nano-banana-2", TextToImage), Some(5));
        assert_eq!(rank("openai:gpt-image-2", TextToImage), Some(7), "a quality level is not a variant");
        assert_eq!(rank("fal:fal-ai/qwen-image", TextToImage), Some(8));
        assert_eq!(rank("fal:openai/gpt-image-2.5/sunburst/edit", ImageEdit), Some(1));
        assert_eq!(rank("fal:fal-ai/nano-banana-pro/edit", ImageEdit), Some(2));
        assert_eq!(rank("fal:fal-ai/flux-2-max/edit", ImageEdit), Some(3));
        assert_eq!(rank("fal:fal-ai/hunyuan-image/v3/instruct/edit", ImageEdit), Some(4));
        assert_eq!(rank("fal:fal-ai/qwen-image-edit-2511", ImageEdit), Some(5));
        assert_eq!(rank("openai:chatgpt-image-latest", ImageEdit), Some(6));
    }

    #[test]
    fn names_keep_only_the_words_that_identify_the_model() {
        let same = |a: &str, b: &str| assert_eq!(words(a), words(b), "{a} / {b}");
        same("qwen-image-2.0-pro-2026-06-22", "qwen-image-2.0-pro");
        same("flux-1-dev-fp8", "flux-1-dev");
        same("HunyuanImage 3.0 Instruct", "fal-ai/hunyuan-image/v3/instruct");
        same("byteplus/dreamina-seedance-2-0-260128", "seedance-2-0");
        assert_ne!(words("veo-3"), words("veo-3.1"));
        assert_ne!(words("gpt-image-1"), words("gpt-image-1.5"));
    }

    #[test]
    fn different_models_never_share_a_score() {
        let boards = Boards::new(&[arena("text_to_video", &["veo-3", "minimax-h3", "ray-3", "flux-3-video", "kling-2.6-pro", "seedance-v1-pro"])]);
        let rank = |id: &str| boards.scores(&model(id, Task::TextToVideo)).first().map(|s| s.rank);
        assert_eq!(rank("fal:fal-ai/veo3.1"), None, "3 is not 3.1");
        assert_eq!(rank("fal:fal-ai/veo3/fast"), None, "fast is a different model");
        assert_eq!(rank("fal:minimax/h3-max/text-to-video"), None);
        assert_eq!(rank("fal:luma/agent/ray/v3.2/text-to-video"), None);
        assert_eq!(rank("fal:blackforestlabs/flux-3-max/text-to-video"), None);
        assert_eq!(rank("fal:blackforestlabs/flux-3/text-to-video/draft"), None);
        assert_eq!(rank("fal:fal-ai/kling-video/v2.6/standard/text-to-video"), None, "standard is not the rated pro tier");
        assert_eq!(rank("fal:fal-ai/bytedance/seedance/v1/lite/text-to-video"), None);
        // An image model is not rated in a video arena.
        assert!(boards.scores(&model("fal:fal-ai/veo3", Task::TextToImage)).is_empty());
    }

    #[test]
    fn one_missing_arena_makes_the_set_stale() {
        let mut arenas: Vec<Arena> = ARENAS.iter().map(|(key, _)| Arena { fetched_at: 5, ..arena(key, &["sora-2"]) }).collect();
        assert_eq!(Boards::new(&arenas).fetched_at, 5);
        arenas.pop();
        assert_eq!(Boards::new(&arenas).fetched_at, 0);
    }
}
