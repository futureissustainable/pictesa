---
name: pictesa
description: Generate, edit and animate images and videos with Pictesa (any provider or local model) from the command line, and find earlier results in the Pictesa library.
---

# Pictesa

Pictesa is installed as the `pictesa` command. Prefer the MCP tools when the client has them; otherwise use the CLI.

1. Pick a model: `pictesa models --output image --featured` (or `--output video`). Ids look like `fal:google/nano-banana-2.1`.
2. Check its inputs and parameters: `pictesa model <id>`.
3. Generate: `pictesa generate "<prompt>" -m <id> [-p key=value ...] [-i role=path ...] [-n count] -o <folder> --json`.
   - Roles: `reference` (edit or style images), `start_frame`, `end_frame`, `video`, `audio`, `mask`.
   - Presets add camera moves or looks: list them in the studio or with the MCP tool `list_presets`; pass `--preset camera/dolly-in`.
4. Read the JSON: each job has `status`, `error` and `files[].path`. Open the files to check the result before reporting.

Rules: generations on paid providers cost money, so make one test run before large batches and never loop on failures. Do not print API keys; they live in Pictesa's settings (`pictesa keys set <provider> api_key` reads the value from stdin).

## Save results from other tools

Save images made outside Pictesa with their prompt and settings:

```bash
pictesa import result.png --prompt-file prompt.txt --origin gemini-api --model google:gemini-3-pro-image --label "Nano Banana Pro" -p resolution=2K --json
```

Use `--prompt TEXT` for short prompts. Add `--negative TEXT` or `--source-url URL` when available. Multiple files share the metadata. Reimports fill missing metadata without replacing saved values. A catalog model ID enables **Reuse settings**. MCP `import_asset` accepts the same metadata (`model_label` for `--label`, `negative_prompt` for `--negative`) with `path` or `url`.
