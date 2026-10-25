# Recipes, presets and skill packs

All three are plain JSON. Built-ins live in `recipes/recipes.json`, `presets/presets.json` and `packs/packs.json`; your own go in the `recipes/`, `presets/` and `packs/` folders inside the Pictesa data folder (`pictesa doctor` shows where). A file holds a JSON array; an entry with the same `id` (a pack: the same `name`) as a built-in replaces it. Restart Pictesa to load changes. The studio shows recipes as Apps.

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
- Color grades are image presets of category `grade` (`grade/teal-orange`, `grade/bleach-bypass`, `grade/portra-400`, ...). The Color grade app offers them; add your own looks the same way.
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

- **Fields** have `type` `text`, `enum` (with `options`), `multi` (several of `options`, sent as a list) or `preset` (with a preset `category`). `required` fields must be filled; `default` fills the rest. `required_without: "scene"` makes a field required unless the input or field `scene` has a value; its `help` is the message people see.
- **Inputs** are files the person provides: `{ "key", "label", "kind": "image"|"video"|"audio", "max", "required" }`.
- **Steps** run in order. `models` is a preference list: the first model that is set up and fits runs, so one recipe works with cloud keys and with a local ComfyUI. A model fits when it makes the step's kind of media, takes a prompt when the step writes one, and has room for the step's files (Face swap needs a model that takes two images). People can pick another model per step.
- `needs` says what to set up when no listed model is ready, for example `"an upscaler, such as Topaz on fal or ESRGAN in ComfyUI"`. The studio and agents show it.
- `prompt` and string `params` use `{{field}}`. A param that is exactly `{{field}}` keeps the field's type, so numbers stay numbers. Empty fields disappear from prompts cleanly, and a list field joins with commas.
- Sections: `{{#key}}...{{/key}}` keeps its text only when the field has a value or the input has files, and `{{^key}}...{{/key}}` only when it has not. Product placement uses them to speak of "the scene from the second image" only when a scene photo was added.
- `each` runs a step once per item, all in one batch, and returns a set; `{{each}}` is the item. Items are fixed texts (the nine shots of Shots) or a lone `{{field}}` that stands for the field's items: the choices of a `multi` field or the lines of a text field (list markers like `1.` and `-` are dropped). A step runs at most 16 times. After another `each` step, items pair up by position, so the Explainer animates each scene's own still. A set keeps the items that worked; the run fails only when none did.
- `picture` names the card picture in the studio: a built-in illustration (`shots`, `angles`, `zooms`, `relight`, `face-swap`, `placement`, `grade`, `upscale`, `ugc`, `hooks`, `explainer`, `cinema`, `character-shot`, `product-hero`, `animate`) or the id of a library image. `character: true` offers a character or brand kit for every step, and `open` names a studio page that runs the recipe with a richer tool.
- Params are adapted to each model: unknown ones are dropped and values snap to the nearest option the model allows (a 5 second request becomes 4 seconds on a model that offers 2, 4 and 6).
- `inputs` of a step take files from the person (`input:<key>`) or the first result of an earlier step (`step:<id>`), with a role: `reference`, `start_frame`, `end_frame`, `video`, `audio` or `mask`.
- Every step is an ordinary generation, so it shows in the studio, the library and the API with its own cost.

Run recipes from the studio's Apps, with `pictesa recipe run <id> -v key=value -i input_key=path [--character <id or name>]`, or with the MCP tool `run_recipe`.

## Skill packs

A pack is a workflow for agents, served as an MCP prompt and by `pictesa pack <name>`:

```json
{
  "name": "hook_variants",
  "title": "Hook variants",
  "description": "Several different openings for the same video, one short clip each.",
  "recipe": "ads/hook-variants",
  "arguments": [
    { "name": "video", "description": "What the video is about.", "required": true },
    { "name": "frame", "description": "The opening frame.", "type": "file", "required": true },
    { "name": "count", "description": "How many openings, from 2 to 8.", "type": "number", "min": 2, "max": 8, "default": "4" }
  ],
  "intro": "Make {{count}} different openings for a video about: {{video}}.",
  "steps": ["Call list_recipes and find \"ads/hook-variants\".", "Call run_recipe with inputs {\"frame\": [{{frame_input}}]}."],
  "rules": ["Each opening costs one clip."]
}
```

- Argument `type`: `text` (default), `number` (with `min` and `max`), `choice` (one of `options`), `file`, `character` or `brand`. Missing, unknown and invalid arguments are refused with the argument named.
- A `file` argument also sets `{{name_input}}`: the JSON an agent passes to `run_recipe` (`{"asset_id": ...}`, `{"url": ...}` or `{"path": ...}`).
- A `character` or `brand` argument (a brand kit only) is found by id or name and sets `{{name}}` to its id, plus `{{name_name}}`, `{{name_images}}`, `{{name_inputs}}` (its images as run inputs) and `{{name_line}}` (a brand kit's prompt line).
- `intro`, `steps` and `rules` use the same templates as recipes; steps are numbered and rules listed after them.
