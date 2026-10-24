# Recipes and presets

Both are plain JSON. Built-ins live in `recipes/recipes.json` and `presets/presets.json`; your own go in the `recipes/` and `presets/` folders inside the Pictesa data folder (`pictesa doctor` shows where). A file holds a JSON array; an entry with the same `id` as a built-in replaces it. Restart Pictesa to load changes.

## Presets

A preset adds words to the prompt and can set parameters.

```json
[
  {
    "id": "camera/dolly-in",
    "name": "Dolly in",
    "category": "camera",
    "output": "video",
    "prompt_suffix": "slow dolly-in toward the subject, smooth forward camera glide",
    "params": {},
    "description": "The camera glides forward toward the subject."
  }
]
```

- `output`: `image` or `video`; the studio shows video presets in video mode.
- `params`: applied only when the chosen model has that parameter, and never over a value the person set.

## Recipes

A recipe is a short pipeline. People fill in its fields and drop in its input files; each step runs one model.

```json
[
  {
    "id": "cinema/shot",
    "name": "Cinema shot",
    "description": "Compose a still with your lens and light, then animate it with a camera move.",
    "output": "video",
    "fields": [
      { "key": "scene", "label": "Scene", "type": "text", "multiline": true, "required": true },
      { "key": "camera", "label": "Camera move", "type": "preset", "category": "camera", "default": "camera/dolly-in" },
      { "key": "lens", "label": "Lens", "type": "enum", "options": ["35mm", "85mm portrait"], "default": "35mm" }
    ],
    "inputs": [],
    "steps": [
      {
        "id": "still",
        "label": "Compose the still",
        "output": "image",
        "models": ["fal:google/nano-banana-2.1", "comfyui:flux3-klein-t2i", "mock:image"],
        "prompt": "{{scene}}, cinematic film still, {{lens}} lens",
        "params": { "aspect_ratio": "16:9" }
      },
      {
        "id": "shot",
        "label": "Shoot the move",
        "output": "video",
        "models": ["fal:kling/v4/image-to-video", "comfyui:ltx25-i2v", "mock:video"],
        "prompt": "{{scene}}",
        "preset": "{{camera}}",
        "params": { "duration": "{{duration}}" },
        "inputs": [{ "role": "start_frame", "from": "step:still" }]
      }
    ]
  }
]
```

- **Fields** have `type` `text`, `enum` (with `options`) or `preset` (with a preset `category`). `required` fields must be filled; `default` fills the rest.
- **Inputs** are files the person provides: `{ "key", "label", "kind": "image"|"video"|"audio", "max", "required" }`.
- **Steps** run in order. `models` is a preference list: the first model whose provider is set up runs, so one recipe works with cloud keys and with a local ComfyUI. People can pick another model per step.
- `prompt` and string `params` use `{{field}}`. A param that is exactly `{{field}}` keeps the field's type, so numbers stay numbers. Empty fields disappear from prompts cleanly.
- Params are adapted to each model: unknown ones are dropped and values snap to the nearest option the model allows (a 5 second request becomes 4 seconds on a model that offers 2, 4 and 6).
- `inputs` of a step take files from the person (`input:<key>`) or the first result of an earlier step (`step:<id>`), with a role: `reference`, `start_frame`, `end_frame`, `video`, `audio` or `mask`.
- Every step is an ordinary generation, so it shows in the studio, the library and the API with its own cost.

Run recipes from the studio, with `pictesa recipe run <id> -v key=value -i input_key=path`, or with the MCP tool `run_recipe`.
