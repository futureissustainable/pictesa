//! Compares the workflow importer's guesses with the hand-written built-in manifests.
//!   cargo run -p pictesa-providers --example import_check
use serde_json::Value;

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workflows/comfy");
    let mut dirs: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        let wf: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("workflow.json")).unwrap()).unwrap();
        let hand: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
        let id = dir.file_name().unwrap().to_string_lossy().to_string();
        match pictesa_providers::comfy_import::infer_manifest(&id, wf.as_object().unwrap()) {
            Ok((m, notes)) => {
                let same = |k: &str| if m[k] == hand[k] { "=" } else { "!=" };
                println!(
                    "{id:<22} out {} {} | tasks {} {} | prompt {} {} vs {} | neg {} | inputs {} vs {} | params {:?} {}",
                    m["output"], same("output"), m["tasks"], same("tasks"), m["prompt"], same("prompt"), hand["prompt"], same("negative_prompt"),
                    m["inputs"].as_array().map(|a| a.iter().map(|i| i["role"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join(",")).unwrap_or_default(),
                    hand["inputs"].as_array().map(|a| a.iter().map(|i| i["role"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join(",")).unwrap_or_default(),
                    m["params"].as_array().map(|a| a.iter().map(|p| p["key"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>()).unwrap_or_default(),
                    if notes.is_empty() { String::new() } else { format!("notes: {}", notes.join("; ")) }
                );
            }
            Err(e) => println!("{id:<22} ERROR {e}"),
        }
    }
}
