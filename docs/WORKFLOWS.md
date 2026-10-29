# ComfyUI workflow templates

Pictesa runs open-weight models through any ComfyUI server: your own GPU, a LAN machine, a server
behind a reverse proxy, or Comfy Cloud. Each **template** is one model in the picker. A template is
a folder with two files:

```
workflows/comfy/<id>/
  workflow.json   the graph, in ComfyUI API format
  manifest.json   what Pictesa shows, and where each prompt, file and setting goes in the graph
```

The folder name is the model id (`comfyui:<id>`). Use lowercase letters, digits, `-`, `_` and `.`.

## Where Pictesa looks

| Source | Location | Notes |
|---|---|---|
| Built-in | `workflows/comfy/` in this repository | Compiled into the binary. Listed in `BUILTIN` in `crates/pictesa-providers/src/comfyui.rs`. |
| Yours | The **Extra workflows folder** setting of the ComfyUI provider, else `PICTESA_WORKFLOWS_DIR`, else `<data dir>/workflows/comfy` | Read on every model refresh. A folder with a built-in's id replaces that built-in. |

`<data dir>` is `PICTESA_DATA_DIR`, else the platform data folder (`~/.local/share/pictesa` on Linux,
`~/Library/Application Support/pictesa` on macOS, `%APPDATA%\pictesa` on Windows).

## Making a template

1. Build and test the workflow in ComfyUI. Keep only what the result needs: remove preview nodes,
   comparison nodes and optional prompt enhancers that load extra models.
2. Export it with **Workflow > Export (API)** and save it as `workflow.json`. A file with `nodes`
   and `links` arrays is the UI format; Pictesa rejects it with a hint.
3. Write `manifest.json` (below). Every node id it names must exist in the graph.
4. Refresh models in Pictesa. The template appears; tags tell you what the server lacks.

## Manifest

```json
{
  "schema": 1,
  "name": "FLUX 3 Klein",
  "description": "Shown under the name in the picker.",
  "vendor": "Black Forest Labs",
  "output": "image",
  "tasks": ["text_to_image"],
  "open_weights": true,
  "license": "FLUX Kommunity License",
  "prompt": [["4", "text"]],
  "negative_prompt": [["5", "text"]],
  "inputs": [],
  "params": [
    {"key": "aspect_ratio", "label": "Aspect ratio", "type": "enum", "options": [{"value": "1:1", "label": "1:1"}], "default": "1:1"},
    {"key": "steps", "label": "Steps", "type": "int", "min": 1, "max": 50, "default": 16, "advanced": true, "bind": [["7", "steps"]]},
    {"key": "seed", "label": "Seed", "type": "seed", "advanced": true, "bind": [["7", "seed"]]}
  ],
  "size": {"width": [["6", "width"]], "height": [["6", "height"]], "megapixels": 1.0, "multiple": 16},
  "batch": [["6", "batch_size"]],
  "max_outputs": 4,
  "outputs": ["9"],
  "requires": {"nodes": ["KSampler", "..."], "models": [{"name": "flux3-image-klein-int8-convrot.safetensors", "directory": "diffusion_models", "url": "https://..."}]},
  "sources": ["https://github.com/Comfy-Org/workflow_templates/blob/main/templates/..."],
  "tags": []
}
```

| Field | Required | Meaning |
|---|---|---|
| `schema` | no | Format version. Only `1` exists. |
| `name`, `description`, `vendor` | `name` | Picker text. `vendor` is the maker of the weights. |
| `output` | yes | `image`, `video` or `audio`. |
| `tasks` | yes | Any of `text_to_image`, `image_edit`, `text_to_video`, `image_to_video`, `video_to_video`, `upscale`, `lip_sync`, `text_to_audio`. |
| `open_weights`, `license` | no | Shown in the picker; `open_weights` lets open-weights-only setups list the model. |
| `prompt` | no | Bindings that receive the prompt text. Empty means the model takes no prompt. |
| `negative_prompt` | no | Bindings for a negative prompt. When the user gives none, the template's own text stays. |
| `inputs` | no | Files the user attaches (see Inputs). |
| `params` | no | Settings (see Parameters). |
| `size` | no | Output width and height (see Size). |
| `batch` | no | Batch-size inputs. With `batch`, one run returns up to `max_outputs` results; without it, one. |
| `outputs` | no | Ids of the saving nodes whose files are the results. Empty means every saved file of the `output` kind. |
| `requires` | no | Node classes and model files the graph needs, with download URLs. Pictesa also derives both from the graph. |
| `sources`, `tags` | no | Provenance links; extra picker tags. |

### Bindings

A binding names a node input: `["7", "steps"]`. The object form adds a value transform:

| Form | Effect |
|---|---|
| `{"node": "55", "input": "length", "frames": {"fps": 24, "fps_param": "fps", "step": 4, "offset": 1}}` | Turns seconds into a frame count: `floor(seconds * fps / step) * step + offset`. `fps_param` names the parameter that holds the frame rate; `fps` is the fallback. Wan uses `4n+1` frames. |
| `{"node": "4", "input": "scale_by", "map": {"2": 0.5, "4": 1.0}}` | Replaces the parameter value (as text) with the mapped value. |

### Parameters

A parameter uses the same fields as the API's `ModelSpec.params` (`key`, `label`, `type`, `options`,
`min`, `max`, `step`, `default`, `advanced`, `help`) plus `bind`. `type` is `enum`, `int`, `float`,
`bool`, `text` or `seed`.

Use the shared keys so presets, the CLI and agents can set the same option on every model:
`aspect_ratio`, `resolution`, `width`, `height`, `seed`, `duration`, `fps`, `guidance`, `steps`,
`strength`, `quality`, `output_format`, `generate_audio`, `camera_motion`, `enhance_prompt`,
`negative_prompt`, `style`, `scale`, `loop`. Model-specific keys (`lightning` in the Wan template)
are fine too.

When a request has no value, the parameter's `default` is used. A `seed` without a value gets a
random seed on every run. `aspect_ratio` and `resolution` need no `bind`: `size` reads them.

### Size

`size` computes the output width and height and writes them to its `width` and `height` bindings.

1. Explicit `width` and `height` parameters win.
2. Otherwise the ratio comes from `aspect_ratio` (`"16:9"`). With `auto` or no value, it comes from
   the first file of the `match_input` role (for example the start frame of image to video), else
   16:9 for video and 1:1 for images.
3. The area comes from `resolutions` when the `resolution` parameter matches a key there
   (`{"720p": 720}` sets the short side in pixels), else from `megapixels` (1.0 = 1024 x 1024).
4. Both sides are rounded to `multiple` (use 16 for most image models, 32 for Wan, 64 for LTX-2.5,
   whose first pass runs at half size).

### Inputs

```json
{"role": "reference", "kind": "image", "nodes": ["11", "61", "62", "63"], "input": "image", "min": 1, "label": "Images to edit"}
```

| Field | Meaning |
|---|---|
| `role` | `reference`, `start_frame`, `end_frame`, `video`, `audio` or `mask`. |
| `kind` | `image`, `video` or `audio`. |
| `nodes` | One loader node per accepted file, in order (`LoadImage`, `LoadVideo`, `LoadAudio`). The count is the maximum. |
| `input` | The loader input that takes the file name. Default: `image` for images, `file` for videos, `audio` for audio. |
| `min` | Files the user must attach. |

Pictesa uploads each file with `POST /api/upload/image` (it accepts videos too) into
`input/pictesa/`, named by content hash so the same file is stored once, and writes
`pictesa/<name>` into the loader. Loaders left without a file are deleted together with every link
to them, so only connect them to optional inputs (`start_image` of `Wan22ImageToVideoLatent`, the
`images.imageN` slots of `ReferenceMultiLatent`).

## Availability tags

On every model refresh Pictesa asks `GET /api/object_info/{class}` for each node class the
templates use (servers without that route get one `GET /api/object_info`) and compares the answers
with each template. Templates with any of these tags are listed but not runnable:

| Tag | Meaning |
|---|---|
| `needs-nodes` | The server lacks node classes (install the custom nodes or update ComfyUI). `meta.missing_nodes` lists them. |
| `needs-models` | Model files are missing. `meta.missing_models` lists them; `meta.downloads` gives the folder and URL of each from `requires.models`. |
| `unavailable` | The server did not answer during the refresh. `meta.unavailable` says why; refresh after starting ComfyUI. |

Each template's description ends with the files it needs and their folders under ComfyUI's
`models/` (`Needs models/vae/flux2-vae.safetensors, ...`). After a check that found missing files it
names only those, with the download source (`Missing on the server: ... Download from
huggingface.co/...`).

A model file kept in a subfolder (`models/diffusion_models/wan/...`) is found and used: the
template's file name is rewritten to the server's path when the job runs.

### Downloading missing models

When ComfyUI runs on the same machine as Pictesa, set its models folder (Settings > Providers >
ComfyUI, or `PICTESA_COMFYUI_MODELS_DIR`). A template tagged `needs-models` then shows **Download
models** with the total size; Pictesa fetches each missing file from its `requires.models` URL into
`models/<directory>/`, resumes a broken download, checks the size and the `sha256` when the manifest
has them, and refreshes the catalog so the model is ready. `POST /api/downloads { model }` starts it
and `GET /api/downloads` reports progress (hosted mode: administrators only).

For a ComfyUI on another machine, run the same download there:
`pictesa download comfyui:flux3-klein-t2i --to D:\ComfyUI\models`. Gated Hugging Face models need
their license accepted on huggingface.co and a token (`HF_TOKEN`, or the Hugging Face token field).

## Running

1. Upload inputs, patch the graph from the manifest, then `POST /api/prompt` with a `client_id`.
2. Poll `GET /api/jobs/{id}` (ComfyUI 0.4 and later, Comfy Cloud). Older servers use
   `GET /api/history/{id}` and `GET /api/queue`. Queued jobs report their queue position.
3. Step progress (`Sampling 12/20`) comes from the `/ws` event stream when it can be opened.
4. Results are fetched from `GET /api/view?filename=&subfolder=&type=`.
5. Cancel removes a queued prompt (`POST /api/queue {"delete": [id]}`) or interrupts it with
   `POST /api/interrupt {"prompt_id": id}` only when it is the one running.

Rejected graphs become readable errors. A missing model file reads, for example:
`the ComfyUI server lacks files this template needs: missing model file: unet_name: 'x.safetensors' not in [] (UNETLoader, node 1)`.

### Comfy Cloud and reverse proxies

Set **Server URL** to `https://cloud.comfy.org` and **API key** to your Comfy API key (`comfyui-...`).
Keys that start with `comfyui-` are sent as `X-API-Key` (and passed to partner nodes in
`extra_data.api_key_comfy_org`); any other value is sent as `Authorization: Bearer` for a reverse
proxy. Comfy Cloud answers `/api/view` with a redirect to a signed URL, which Pictesa downloads
without the key.

## Built-in templates

All built-ins come from the official [Comfy-Org/workflow_templates](https://github.com/Comfy-Org/workflow_templates)
(templates package 0.11.89, ComfyUI 0.42.1). Each graph was exported to API format with the
ComfyUI frontend, then trimmed: optional LLM prompt enhancers removed (they load another 5 to 10 GB
model), the `ResolutionSelector` replaced by computed width and height, and 4-step LoRA switches set
on where the template has them. Sizes are the total weights to download. `manifest.json` lists every
file with its folder, URL and SHA-256.

| Id | Model | Task | Weights | License |
|---|---|---|---|---|
| `flux3-klein-t2i` | FLUX 3 Klein, distilled int8, 16 steps | text to image | 11.0 GB | FLUX Kommunity License, gated repo |
| `flux3-klein-edit` | FLUX 3 Klein Edit, up to 4 references | image edit | 11.0 GB | FLUX Kommunity License, gated repo |
| `qwen-image-2512` | Qwen-Image 2512 fp8, 4-step Lightning LoRA | text to image | 32 GB | Apache-2.0 |
| `qwen-image-21` | Qwen Image 2.1 int8, 25 steps | text to image | 17.3 GB | Qwen Research License (non-commercial) |
| `qwen-image-21-turbo` | Qwen Image 2.1 Turbo, 8 steps | text to image | 17.3 GB | Qwen Research License (non-commercial) |
| `ltx25-t2v` | LTX-2.5 22B distilled int8, with audio | text to video | 39.7 GB | LTX-2.x Community License, gated repo |
| `ltx25-i2v` | LTX-2.5 22B distilled int8, with audio | image to video | 39.7 GB | LTX-2.x Community License, gated repo |
| `wan22-i2v` | Wan 2.2 14B fp8, 4-step lightx2v LoRAs | image to video | 38.0 GB | Apache-2.0 |
| `wan22-5b` | Wan 2.2 TI2V 5B, optional start frame | text or image to video | 18.1 GB | Apache-2.0 |
| `esrgan-upscale` | Real-ESRGAN x4plus, 2x or 4x | image upscale | 67 MB | BSD-3-Clause |
| `esrgan-video-upscale` | Real-ESRGAN x4plus per frame, audio kept | video upscale | 67 MB | BSD-3-Clause |

The graphs are validated against ComfyUI 0.42.1 but not yet run on a GPU. The int8 and fp8 builds
target consumer GPUs: FLUX 3 Klein fits a 12 GB card, and the larger models rely on ComfyUI's weight
offloading, which needs a lot of system RAM for the 14B to 22B models. The 4-step LoRA switches
(`lightning`) are on by default for Wan 2.2 14B and Qwen-Image 2512.

To add a built-in: create the folder, add its id to `BUILTIN` in `comfyui.rs`, and run
`cargo test -p pictesa-providers comfyui`. The tests parse every built-in, check that each binding
names an existing node input, and check the node classes against a recorded `object_info`.

## Testing against a real server

```
COMFYUI_URL=http://127.0.0.1:8188 cargo test -p pictesa-providers --test comfyui_live -- --ignored --nocapture
```

`esrgan_upscale` needs `RealESRGAN_x4plus.safetensors` in `models/upscale_models`. `text_to_image`
runs the template named by `COMFYUI_LIVE_T2I`, for example a one-step SD 1.5-class model in
`PICTESA_WORKFLOWS_DIR`, and also checks cancelling a queued job. Both run on a CPU-only server.
