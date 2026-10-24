// Makes sure the embedded UI folder exists so a fresh clone builds before `pnpm build` runs.
fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    if !dist.join("index.html").exists() {
        let _ = std::fs::create_dir_all(&dist);
        let _ = std::fs::write(
            dist.join("index.html"),
            "<!doctype html><meta charset=utf-8><title>Pictesa</title><body style=\"font:16px system-ui;padding:3rem\"><h1>Pictesa is running</h1><p>The web UI was not built into this binary. Run <code>pnpm --dir web install &amp;&amp; pnpm --dir web build</code>, then rebuild. The API, MCP endpoint and CLI work already.</p>",
        );
    }
    println!("cargo:rerun-if-changed=../../web/dist");
}
