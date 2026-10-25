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

## Apps (recipes)

One command runs a whole app, and each step picks the first set-up model that fits:

```bash
pictesa recipe list                                                   # "ok" when every step has a model
pictesa recipe run photo/shots -i photo=./scene.png -o ./out          # nine shots of one scene
pictesa recipe run photo/angles -i photo=./chair.png -v 'angles=["view from behind","low angle looking up"]'
pictesa recipe run photo/face-swap -i face=./face.png -i target=./photo.png
pictesa recipe run product/placement -i product=./bottle.png -v where="on a marble counter"
pictesa recipe run photo/color-grade -i photo=./street.png -v look=grade/bleach-bypass
```

Others: `photo/zooms`, `photo/relight`, `photo/upscale`, `ads/ugc`, `ads/hook-variants`, `video/explainer`, `cinema/shot`, `cinema/character-shot`, `product/hero`, `photo/animate`. When a step has no model, the error says what to set up; tell the user. MCP clients use `list_recipes` and `run_recipe` (its `results` holds the final files).

## Skill packs

Ready-made workflows for bigger jobs. With MCP, call `prompts/list` and `prompts/get`; with the CLI, print the same steps and follow them:

```bash
pictesa pack                                         # ugc_ad, hook_variants, explainer, faceless_short, brand_kit
pictesa pack ugc_ad -a product="a ceramic pour-over set" -a product_image=./set.png
pictesa pack explainer -a topic="how a heat pump works" -a scenes=5
pictesa pack brand_kit -a brand="Oak and Co" -a brief="three product shots for spring"
```

A brand kit is a saved character of kind brand (Characters page): its images, colors and words apply to every step when you pass `--character <id or name>` to `recipe run` or `character` to the MCP tools.

## Save results from other tools

Save images made outside Pictesa with their prompt and settings:

```bash
pictesa import result.png --prompt-file prompt.txt --origin gemini-api --model google:gemini-3-pro-image --label "Nano Banana Pro" -p resolution=2K --json
```

Use `--prompt TEXT` for short prompts. Add `--negative TEXT` or `--source-url URL` when available. Multiple files share the metadata. Reimports fill missing metadata without replacing saved values. A catalog model ID enables **Reuse settings**. MCP `import_asset` accepts the same metadata (`model_label` for `--label`, `negative_prompt` for `--negative`) with `path` or `url`.
