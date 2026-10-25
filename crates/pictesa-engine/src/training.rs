//! Training and LoRA ownership are validated before the ordinary job transaction reserves money.
use crate::{store, Asset, CreateGeneration, Created, Engine, EngineError, JobInput, Result};
use pictesa_core::{lora::TrainingOptions, GenerationRequest, InputRole, MediaKind, ModelSpec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TrainRequest {
    pub trainer: String,
    #[serde(default)]
    pub asset_ids: Vec<String>,
    #[serde(flatten)]
    pub options: TrainingOptions,
}
#[derive(Debug, Clone, Serialize)]
pub struct Lora {
    pub asset: Asset,
    pub character_id: Option<String>,
    pub family: String,
    pub trigger_word: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LoraSelection {
    pub asset_id: String,
    #[serde(default = "one")]
    pub strength: f64,
}
fn one() -> f64 {
    1.0
}

impl TrainRequest {
    fn generation(self) -> Result<CreateGeneration> {
        if self.options.character.is_some() && !self.asset_ids.is_empty() {
            return Err(EngineError::invalid(
                "Choose a character or library images, not both.",
                Some("asset_ids"),
            ));
        }
        Ok(CreateGeneration {
            model: self.trainer,
            params: json!({"_training":self.options})
                .as_object()
                .unwrap()
                .clone(),
            inputs: self
                .asset_ids
                .into_iter()
                .map(|asset_id| JobInput {
                    asset_id,
                    role: InputRole::Reference,
                })
                .collect(),
            ..Default::default()
        })
    }
}
impl Engine {
    pub async fn train(&self, user: &str, req: TrainRequest) -> Result<Created> {
        self.create_generation(user, req.generation()?).await
    }

    pub async fn estimate_training(
        &self,
        user: &str,
        req: TrainRequest,
    ) -> Result<crate::batches::BatchEstimate> {
        self.estimate_batch(
            user,
            crate::batches::BatchRequest {
                generation: req.generation()?,
                ..Default::default()
            },
        )
        .await
    }

    pub async fn loras(
        &self,
        user: &str,
        family: Option<&str>,
        character: Option<&str>,
    ) -> Result<Vec<Lora>> {
        let rows:Vec<(String,Option<String>,String,String)>=sqlx::query_as("SELECT l.asset_id,l.character_id,l.family,l.trigger_word FROM loras l JOIN assets a ON a.id=l.asset_id AND a.user_id=l.user_id WHERE l.user_id=? AND a.deleted_at IS NULL AND (? IS NULL OR l.family=?) AND (? IS NULL OR l.character_id=?) ORDER BY a.id DESC")
            .bind(user).bind(family).bind(family).bind(character).bind(character).fetch_all(&self.inner.db).await?;
        let mut out = Vec::new();
        for (id, character_id, family, trigger_word) in rows {
            out.push(Lora {
                asset: store::asset_row(&self.inner.db, user, &id)
                    .await?
                    .to_asset(),
                character_id,
                family,
                trigger_word,
            });
        }
        Ok(out)
    }

    pub(crate) async fn validate_training(
        &self,
        user: &str,
        spec: &ModelSpec,
        req: &mut CreateGeneration,
        raw: Option<Value>,
    ) -> Result<Option<Value>> {
        if spec.output != MediaKind::Lora {
            if raw.is_some() {
                return Err(EngineError::invalid(
                    "Choose a training model.",
                    Some("trainer"),
                ));
            }
            return Ok(None);
        }
        if spec.provider == "ai-toolkit" && self.mode() == crate::Mode::Hosted {
            return Err(EngineError::Forbidden(
                "Local training is only available in local mode.".into(),
            ));
        }
        let opts: TrainingOptions = serde_json::from_value(raw.ok_or_else(|| {
            EngineError::invalid("Use the training form or API.", Some("_training"))
        })?)
        .map_err(|e| {
            EngineError::invalid(format!("Invalid training settings: {e}"), Some("_training"))
        })?;
        opts.validate()
            .map_err(|e| EngineError::invalid(e, Some("_training")))?;
        if pictesa_core::lora::family(&spec.id, &spec.meta) != Some(opts.family.as_str()) {
            return Err(EngineError::invalid(
                "The trainer and base model family do not match.",
                Some("family"),
            ));
        }
        if req.count.unwrap_or(1) != 1 || !req.compare.is_empty() || req.preset.is_some() {
            return Err(EngineError::invalid(
                "Training runs one LoRA at a time, without a preset or comparison.",
                Some("trainer"),
            ));
        }
        if let Some(cid) = opts.character.as_deref() {
            let c = self.character(user, cid).await?;
            if req.inputs.is_empty() {
                req.inputs = c
                    .assets
                    .into_iter()
                    .map(|a| JobInput {
                        asset_id: a.id,
                        role: InputRole::Reference,
                    })
                    .collect();
            }
        }
        let mut hashes = std::collections::HashSet::new();
        let mut total = 0i64;
        for input in &req.inputs {
            let row = store::asset_row(&self.inner.db, user, &input.asset_id).await?;
            let (w, h) = (row.width.unwrap_or(0), row.height.unwrap_or(0));
            if input.role != InputRole::Reference
                || row.kind != "image"
                || row.deleted_at.is_some()
                || !matches!(row.mime.as_str(), "image/png" | "image/jpeg" | "image/webp")
            {
                return Err(EngineError::invalid(
                    "Choose PNG, JPEG or WebP images from your library.",
                    Some("asset_ids"),
                ));
            }
            if w < 256
                || h < 256
                || w > 8192
                || h > 8192
                || w * h > 24_000_000
                || row.size > 32 * 1024 * 1024
            {
                return Err(EngineError::invalid("Training images need at least 256 px per side, at most 8192 px per side, 24 megapixels and 32 MiB each.",Some("asset_ids")));
            }
            if !hashes.insert(row.hash) {
                return Err(EngineError::invalid(
                    "Choose distinct training images.",
                    Some("asset_ids"),
                ));
            }
            total += row.size;
        }
        if !(3..=100).contains(&hashes.len()) || total > 512 * 1024 * 1024 {
            return Err(EngineError::invalid(
                "Choose 3 to 100 distinct images, up to 512 MiB in total.",
                Some("asset_ids"),
            ));
        }
        let raw = json!(opts);
        if spec.provider == "fal" {
            let provider_req = GenerationRequest {
                job_id: String::new(),
                model: spec.remote_id().into(),
                prompt: String::new(),
                negative_prompt: None,
                params: json!({"_training":raw}).as_object().unwrap().clone(),
                inputs: vec![],
                num_outputs: 1,
            };
            pictesa_providers::fal::training_body(
                spec,
                &provider_req,
                "https://example.invalid/images.zip",
            )
            .map_err(|e| EngineError::invalid(e.to_string(), Some("_training")))?;
        }
        Ok(Some(raw))
    }

    pub(crate) async fn validate_loras(
        &self,
        user: &str,
        spec: &ModelSpec,
        prompt: &mut String,
        raw: Option<Value>,
    ) -> Result<Option<Value>> {
        let Some(raw) = raw else {
            return Ok(None);
        };
        let selections: Vec<LoraSelection> = serde_json::from_value(raw).map_err(|_| {
            EngineError::invalid("LoRAs need asset_id and strength.", Some("loras"))
        })?;
        if selections.is_empty() {
            return Ok(None);
        }
        if selections.len() > 4
            || spec.meta.get("supports_loras").and_then(Value::as_bool) != Some(true)
        {
            return Err(EngineError::invalid(
                "This model does not accept these LoRAs (maximum 4).",
                Some("loras"),
            ));
        }
        if self.mode() == crate::Mode::Hosted
            && matches!(spec.provider.as_str(), "comfyui" | "sdcpp")
        {
            return Err(EngineError::Forbidden(
                "Shared local LoRA directories are only available in local mode.".into(),
            ));
        }
        let family = pictesa_core::lora::family(&spec.id, &spec.meta).ok_or_else(|| {
            EngineError::invalid("The model's LoRA family is unknown.", Some("loras"))
        })?;
        let available = self.loras(user, Some(family), None).await?;
        let mut seen = std::collections::HashSet::new();
        for selection in &selections {
            if !selection.strength.is_finite()
                || !(0.0..=2.0).contains(&selection.strength)
                || !seen.insert(&selection.asset_id)
            {
                return Err(EngineError::invalid(
                    "Choose each LoRA once, with strength from 0 to 2.",
                    Some("loras"),
                ));
            }
            let lora = available
                .iter()
                .find(|l| l.asset.id == selection.asset_id)
                .ok_or_else(|| {
                    EngineError::invalid(
                        "That LoRA is unavailable or uses a different model family.",
                        Some("loras"),
                    )
                })?;
            *prompt = pictesa_core::lora::with_trigger(prompt, &lora.trigger_word);
        }
        Ok(Some(json!(selections)))
    }

    pub(crate) async fn lora_request_params(
        &self,
        user: &str,
        provider: &str,
        params: &mut Map<String, Value>,
    ) -> Result<()> {
        let Some(raw) = params.get("loras").cloned() else {
            return Ok(());
        };
        let selected: Vec<LoraSelection> =
            serde_json::from_value(raw).map_err(anyhow::Error::from)?;
        let mut mapped = Vec::new();
        for lora in selected {
            let row = store::asset_row(&self.inner.db, user, &lora.asset_id).await?;
            if row.deleted_at.is_some() || row.kind != "lora" {
                return Err(EngineError::invalid(
                    "A LoRA was removed from the library.",
                    Some("loras"),
                ));
            }
            let mut path = self.inner.files.path(&row.hash, &row.ext);
            if matches!(provider, "comfyui" | "sdcpp") {
                if self.mode() == crate::Mode::Hosted {
                    return Err(EngineError::Forbidden(
                        "Local LoRA directories require local mode.".into(),
                    ));
                }
                let p = self
                    .inner
                    .catalog
                    .provider(provider)
                    .ok_or_else(|| EngineError::not_found("provider"))?;
                let ctx = self.ctx_for(user, p.as_ref()).await?;
                let dir = std::path::PathBuf::from(ctx.get("lora_dir").ok_or_else(|| {
                    EngineError::invalid("Set the provider's shared LoRA directory.", Some("loras"))
                })?);
                let name = format!("pictesa-{}.safetensors", row.hash);
                tokio::fs::create_dir_all(&dir)
                    .await
                    .map_err(anyhow::Error::from)?;
                let dest = dir.join(&name);
                if !dest.exists() {
                    tokio::fs::copy(&path, &dest)
                        .await
                        .map_err(anyhow::Error::from)?;
                }
                path = std::path::PathBuf::from(name);
            }
            let url = if provider == "fal" {
                self.inner
                    .cfg
                    .public_url
                    .as_ref()
                    .map(|base| crate::queue::signed_file_url(self, base, &row.id))
            } else {
                None
            };
            mapped.push(json!({"path":path,"url":url,"strength":lora.strength}));
        }
        params.insert("loras".into(), json!(mapped));
        Ok(())
    }

    pub(crate) async fn register_lora(&self, job: &store::JobRow, asset: &Asset) -> Result<()> {
        if asset.kind != MediaKind::Lora {
            return Ok(());
        }
        let opts: TrainingOptions = serde_json::from_value(
            job.params_map()
                .get("_training")
                .cloned()
                .unwrap_or(Value::Null),
        )
        .map_err(anyhow::Error::from)?;
        let character = match opts.character {
            Some(id) if self.character(&job.user_id, &id).await.is_ok() => Some(id),
            _ => None,
        };
        sqlx::query("INSERT INTO loras(asset_id,user_id,character_id,family,trigger_word) VALUES(?,?,?,?,?)")
            .bind(&asset.id).bind(&job.user_id).bind(character).bind(opts.family).bind(opts.trigger_word).execute(&self.inner.db).await?;
        self.emit(&job.user_id, "lora", json!({"asset_id":asset.id}));
        Ok(())
    }
}
