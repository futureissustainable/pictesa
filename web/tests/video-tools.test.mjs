import { test } from "node:test";
import assert from "node:assert/strict";
import { videoFit, videoToolModels } from "../src/lib/video-tools.ts";

const model = (id, output, extra = {}) => ({
  id,
  name: id,
  provider: "fal",
  output,
  tasks: [],
  inputs: [{ kind: "video", role: "video", min: 1, max: 1 }],
  params: [],
  prompt: true,
  configured: true,
  complete: true,
  tags: [],
  ...extra,
});

test("video tool models require a configured provider and compatible resolved inputs", () => {
  const topaz = model("fal:topaz/upscale/video", "video");
  const generic = model("fal:video-upscaler", "video");
  assert.deepEqual(
    videoToolModels("upscale", [generic, topaz, model("fal:seedvr", "video", { configured: false })]).map((m) => m.id),
    [topaz.id, generic.id],
  );
  assert.equal(videoFit("upscale", model("fal:topaz/image", "image")), null);
  assert.equal(videoFit("upscale", model("fal:topaz/video/interpolation", "video")), null);
  assert.equal(
    videoFit("upscale", {
      ...topaz,
      inputs: [{ kind: "image", role: "reference", min: 1, max: 1 }],
    }),
    null,
  );
  assert.equal(videoFit("extend", model("fal:video/extend", "video", { complete: false, inputs: [] })) !== null, true);
  assert.equal(videoFit("extend", model("fal:video/edit", "video")), null);
  assert.equal(
    videoFit(
      "extend",
      model("fal:video/extend", "video", {
        inputs: [
          { kind: "video", role: "video", min: 1, max: 1 },
          { kind: "audio", role: "audio", min: 1, max: 1 },
        ],
      }),
    ),
    null,
  );
});

test("cloud tools exclude models that require more inputs than the editor can supply", () => {
  const extend = model("fal:extend-video", "video", { inputs: [{ kind: "video", role: "video", min: 2, max: 2 }] });
  assert.equal(videoFit("extend", extend), null);
});

test("sound accepts video with generated sound and audio-only results, without admitting unrelated video models", () => {
  assert.notEqual(videoFit("sound", model("fal:mmaudio-v2", "video", { complete: false, inputs: [] })), null);
  assert.notEqual(videoFit("sound", model("fal:video-to-audio", "audio")), null);
  assert.equal(videoFit("sound", model("fal:mmaudio-v2/text-to-audio", "audio", { inputs: [] })), null);
  assert.equal(videoFit("sound", model("fal:video-edit", "video")), null);
  assert.equal(videoFit("trim", model("fal:trim", "video")), null);
  assert.notEqual(videoFit("trim", model("local:trim", "video")), null);
});
