import { test } from "node:test";
import assert from "node:assert/strict";
import { cleanName, groupFamilies, intentOf, pickVariant, variantFor } from "../src/lib/families.ts";

const model = (id, family, tasks, extra = {}) => ({
  id,
  name: id,
  provider: id.split(":")[0],
  family,
  output: "image",
  tasks,
  inputs: [],
  params: [],
  prompt: true,
  negative_prompt: false,
  max_outputs: 1,
  open_weights: false,
  configured: true,
  ...extra,
});

test("names lose provider prefixes and task endings", () => {
  assert.equal(cleanName("Google: Nano Banana 2.1"), "Nano Banana 2.1");
  assert.equal(cleanName("MAI Image 2.5 Pro (Edit)"), "MAI Image 2.5 Pro");
  assert.equal(cleanName("Meta Muse Image Text to Image"), "Meta Muse Image");
  assert.equal(cleanName("GPT Image 2 API"), "GPT Image 2");
  assert.equal(cleanName("Nano Banana 2 (preview)"), "Nano Banana 2");
  assert.equal(cleanName("Wan 2.7 I2V"), "Wan 2.7");
});

test("one row per model across providers and tasks, named by its providers", () => {
  const families = groupFamilies([
    model("fal:google/nano-banana-2.1", "2.1 banana nano", ["text_to_image"], { name: "Nano Banana 2.1", vendor: "Google" }),
    model("fal:google/nano-banana-2.1/edit", "2.1 banana nano", ["image_edit"], { name: "Nano Banana 2.1 Edit", vendor: "Google" }),
    model("openrouter:google/gemini-nano-banana-2.1", "2.1 banana nano", ["text_to_image", "image_edit"], { name: "Google: Nano Banana 2.1", vendor: "google" }),
    model("fal:seedream/v5/flash", "5 flash seedream", ["text_to_image"], { name: "Seedream" }),
    model("fal:seedream/v5/lite", "5 lite seedream", ["text_to_image"], { name: "Seedream" }),
    model("fal:fal-ai/flux/schnell", "flux schnell", ["text_to_image"], { name: "FLUX.1 [schnell]" }),
    model("fal:fal-ai/flux-1/schnell", "1 flux schnell", ["text_to_image"], { name: "FLUX.1 [schnell]" }),
    model("fal:fal-ai/nano-banana", "2.5 flash gemini", ["text_to_image"], { name: "Nano Banana" }),
    model("google:gemini-2.5-flash-image", "2.5 flash gemini", ["text_to_image"], { name: "Gemini 2.5 Flash Image" }),
    model("openrouter:google/gemini-2.5-flash-image", "2.5 flash gemini", ["text_to_image"], { name: "Google: Nano Banana" }),
  ]);
  const names = families.map((family) => family.name).sort();
  assert.deepEqual(names, ["FLUX.1 [schnell]", "Nano Banana", "Nano Banana 2.1", "Seedream 5 Flash", "Seedream 5 Lite"]);
  const nano = families.find((family) => family.name === "Nano Banana 2.1");
  assert.equal(nano.variants.length, 3);
  assert.equal(nano.maker, "Google", "a written maker beats an id prefix");
});

test("the member fits what the composer is doing", () => {
  const t2i = model("fal:nano", "nano", ["text_to_image"], { pricing: { unit: "per_image", usd: 0.04 } });
  const edit = model("fal:nano/edit", "nano", ["image_edit"], { pricing: { unit: "per_image", usd: 0.04 } });
  const other = model("openrouter:nano", "nano", ["text_to_image", "image_edit"], { pricing: { unit: "variable", note: "tokens" } });
  const keyless = model("comfy_router:nano", "nano", ["text_to_image", "image_edit"], { configured: false });
  const [family] = groupFamilies([keyless, other, edit, t2i]);
  assert.equal(pickVariant(family, "text_to_image").id, "fal:nano", "set up and priced first");
  assert.equal(pickVariant(family, "image_edit").id, "fal:nano/edit");
  assert.equal(pickVariant(family, "image_edit", other).id, "openrouter:nano", "the current member stays when it fits");
  assert.equal(pickVariant(family, "image_edit", null, ["openrouter:nano"]).id, "openrouter:nano", "a recently used member wins");
  const all = [t2i, edit, other, keyless];
  assert.equal(variantFor(all, t2i, "image_edit").id, "fal:nano/edit", "same provider first");
  assert.equal(variantFor(all, edit, "text_to_image").id, "fal:nano");
  assert.equal(variantFor([t2i], t2i, "image_edit"), null);
});

test("a reference image means editing", () => {
  const asset = { id: "a" };
  assert.equal(intentOf("image", []), "text_to_image");
  assert.equal(intentOf("image", [{ key: "1", role: "reference", asset }]), "image_edit");
  assert.equal(intentOf("video", [{ key: "1", role: "start_frame", asset }]), "image_to_video");
  assert.equal(intentOf("video", []), "text_to_video");
});
