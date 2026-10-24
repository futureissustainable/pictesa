# Writing a Pictesa provider

Read first: `crates/pictesa-core/src/{model.rs,provider.rs,request.rs,schema.rs,error.rs}`, the reference gateway `crates/pictesa-providers/src/fal.rs`, helpers in `util.rs`, and `docs/API.md`.

Rules
- One module per provider in `crates/pictesa-providers/src/<id>.rs`, a unit struct implementing `pictesa_core::Provider`.
- Register it in `lib.rs` only by replacing your batch's marker comment lines (`// BATCH-A-MODULES ...` / `// BATCH-A-REGISTRY`) with `pub mod x;` and `Arc::new(x::X),` lines, keeping the marker comment below your lines. Re-read lib.rs right before editing; other workers edit other markers in parallel.
- Do not edit pictesa-core, fal.rs, other providers' files, or Cargo.toml dependencies beyond adding a crate you truly need to `crates/pictesa-providers/Cargo.toml` (prefer what is there: reqwest, serde_json, base64, hmac, sha2, ulid, url, tokio).
- `info()`: id (lowercase), name, kind, credential fields with the provider's usual env var names, signup_url, docs_url, sensible max_concurrency.
- `models()`: a static catalog of the provider's CURRENT (October 2026) image and video models with correct remote ids, inputs (InputSlot roles: reference, start_frame, end_frame, video, audio, mask), params using the shared keys in `model.rs::keys` wherever they fit (aspect_ratio, resolution, duration, seed, generate_audio, ...), max_outputs, pricing when published, vendor. Verify ids against the provider's own docs or free model-listing endpoint; never invent ids. Use `discover()` when the provider has a free catalog API (Replicate, OpenRouter, Google, OpenAI list endpoints).
- `submit()` maps the shared params to the provider's request, uploads or inlines `req.inputs` (MediaInput helpers: data_uri, base64, url_or_data_uri; `url` is set when a public URL exists), and returns `Submission::Pending(handle)` for async APIs or `Submission::Done(outputs)` for synchronous ones. Store everything polling needs in `RemoteHandle.data` (it is persisted, so polling survives restarts).
- `poll()` maps the provider's states to `PollStatus` (report real progress or queue position when given; never invent percentages). Map failures with `util::failure` / `ProviderError::from_http` so the engine retries only transient errors and rate limits.
- `check()` must be the cheapest authenticated call (list models, account info). It must never spend money.
- Outputs: return `Output::Url` (with auth headers if the download needs them) or `Output::Bytes` for base64 answers. The engine downloads, hashes, thumbnails and stores them.

Testing
- `cargo check -p pictesa-providers` (env: `PATH=$HOME/.cargo/bin:$PATH CARGO_TARGET_DIR=$HOME/build/pictesa-target`, run builds with `nice -n 10`; the target dir is shared, so cargo may wait for a lock).
- Add focused unit tests for request mapping and response parsing with recorded JSON samples (no network) in your module (`#[cfg(test)]`).
- Free live probes with the dev tool: `cargo run -p pictesa-providers --example probe -- <id> [model] --check`. Pass the provider's key in its environment variable for that one command, e.g. `OPENAI_API_KEY=... cargo run ...`. Never print, log or commit a key or any part of it.
- NEVER run `--submit` or any call that generates media or spends credits. Paid end-to-end tests happen later with the owner's approval.
- Do not commit; the orchestrator commits.
