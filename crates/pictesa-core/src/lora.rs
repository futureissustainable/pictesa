//! Shared training contracts and conservative model-family matching.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TrainingOptions {
    pub character: Option<String>,
    pub trigger_word: String,
    pub family: String,
    pub steps: u32,
    pub rank: u32,
    pub learning_rate: f64,
}
impl Default for TrainingOptions {
    fn default() -> Self {
        Self {
            character: None,
            trigger_word: String::new(),
            family: "flux1".into(),
            steps: 1000,
            rank: 16,
            learning_rate: 0.0001,
        }
    }
}
impl TrainingOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.trigger_word.is_empty()
            || self.trigger_word.len() > 64
            || !self
                .trigger_word
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err("Trigger word needs 1 to 64 letters, numbers or underscores.".into());
        }
        if !matches!(
            self.family.as_str(),
            "flux1" | "qwen_image" | "qwen_image_2512" | "wan21"
        ) {
            return Err("Choose FLUX.1, Qwen Image, Qwen Image 2512 or Wan 2.1.".into());
        }
        if !(100..=10000).contains(&self.steps) {
            return Err("Training needs 100 to 10,000 steps.".into());
        }
        if !(4..=128).contains(&self.rank) {
            return Err("Rank must be 4 to 128.".into());
        }
        if !self.learning_rate.is_finite() || !(0.000001..=0.001).contains(&self.learning_rate) {
            return Err("Learning rate must be 0.000001 to 0.001.".into());
        }
        Ok(())
    }
}

/// Unknown or newer architectures do not inherit a previous architecture's LoRAs.
pub fn family(id: &str, meta: &Value) -> Option<&'static str> {
    if let Some(f) = meta.get("lora_family").and_then(Value::as_str) {
        return match f {
            "flux1" => Some("flux1"),
            "qwen_image" => Some("qwen_image"),
            "qwen_image_2512" => Some("qwen_image_2512"),
            "wan21" => Some("wan21"),
            _ => None,
        };
    }
    let name = id.to_ascii_lowercase();
    if name.contains("qwen-image-2512") || name.contains("qwen_image_2512") {
        return Some("qwen_image_2512");
    }
    if [
        "flux2",
        "flux-2",
        "flux/2",
        "flux3",
        "flux-3",
        "klein",
        "kontext",
        "qwen-image-2",
        "qwen-image-3",
        "qwen_image_2",
        "qwen-image-edit",
        "wan-2.2",
        "wan2.2",
        "wan-2.6",
        "wan-3",
    ]
    .iter()
    .any(|s| name.contains(s))
    {
        return None;
    }
    if name.contains("flux") {
        Some("flux1")
    } else if name.contains("qwen-image") || name.contains("qwen_image") {
        Some("qwen_image")
    } else if name.contains("wan-2.1") || name.contains("wan2.1") || name.contains("wan-trainer") {
        Some("wan21")
    } else {
        None
    }
}

pub fn with_trigger(prompt: &str, trigger: &str) -> String {
    if prompt
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|s| s.eq_ignore_ascii_case(trigger))
    {
        prompt.into()
    } else {
        format!("{trigger} {prompt}").trim_end().into()
    }
}

/// tqdm uses both CR and LF and prefixes the fraction with a percentage and bar.
pub fn progress(line: &str) -> Option<f32> {
    line.split_whitespace().find_map(|word| {
        let (a, b) = word.trim_matches('|').split_once('/')?;
        let done = a.parse::<u32>().ok()?;
        let total = b
            .trim_end_matches(|c: char| !c.is_ascii_digit())
            .parse::<u32>()
            .ok()?;
        (total > 0 && done <= total).then_some(done as f32 / total as f32)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn training_limits_reject_invalid_settings() {
        let good = TrainingOptions {
            trigger_word: "maya_person".into(),
            ..Default::default()
        };
        assert!(good.validate().is_ok());
        for bad in [
            TrainingOptions {
                trigger_word: "../../x".into(),
                ..good.clone()
            },
            TrainingOptions {
                steps: 0,
                ..good.clone()
            },
            TrainingOptions {
                rank: 0,
                ..good.clone()
            },
            TrainingOptions {
                learning_rate: f64::NAN,
                ..good.clone()
            },
            TrainingOptions {
                family: "flux2".into(),
                ..good.clone()
            },
        ] {
            assert!(bad.validate().is_err());
        }
    }
    #[test]
    fn tqdm_progress_and_trigger_boundaries() {
        assert_eq!(
            progress("maya:  25%|██ | 250/1000 [00:30<01:20, 2.1it/s]"),
            Some(0.25)
        );
        assert_eq!(progress("loading model 14/0"), None);
        assert_eq!(progress("101/100"), None);
        assert_eq!(with_trigger("MAYA at the lake", "maya"), "MAYA at the lake");
        assert_eq!(
            with_trigger("mayas at the lake", "maya"),
            "maya mayas at the lake"
        );
        assert_eq!(family("fal:flux-2/lora", &Value::Null), None);
        assert_eq!(family("fal:flux-lora", &Value::Null), Some("flux1"));
    }
}
