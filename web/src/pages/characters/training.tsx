import { useEffect, useState, type FormEvent } from "react";
import { api, errorMessage } from "../../lib/api";
import { useCharacters } from "../../lib/characters";
import { useFullModel } from "../../lib/catalog";
import { familyLabel } from "../../lib/loras";
import { money } from "../../lib/format";
import { addJobs } from "../../lib/jobs";
import { Link } from "../../lib/router";
import type { Asset, Character, ModelSpec, TrainRequest } from "../../lib/types";
import { Dialog } from "../../ui/dialog";
import { Spinner } from "../../ui/icon";
import { LibraryPicker } from "../create/library-picker";

export function TrainingDialog({ character, onClose }: { character?: Character; onClose(): void }) {
  const characters = useCharacters().data ?? [];
  const [source, setSource] = useState(character?.id ?? "images");
  const [images, setImages] = useState<Asset[]>([]);
  const [picking, setPicking] = useState(false);
  const [trainers, setTrainers] = useState<ModelSpec[]>([]);
  const [trainer, setTrainer] = useState("");
  const [family, setFamily] = useState("flux1");
  const [trigger, setTrigger] = useState("");
  // Number fields keep what was typed; the values are checked, not forced, while typing.
  const [stepsText, setSteps] = useState("1000");
  const [rankText, setRank] = useState("16");
  const [rateText, setRate] = useState("0.0001");
  const steps = Number(stepsText);
  const rank = Number(rankText);
  const rate = Number(rateText);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [estimate, setEstimate] = useState<{ value: number | null; body: TrainRequest } | null>(null);
  useEffect(() => { void api.models({ output: "lora", configured: true }).then(({ models }) => { setTrainers(models); setTrainer(models[0]?.id ?? ""); }).catch((e) => setError(errorMessage(e))); }, []);
  const listed = trainers.find((m) => m.id === trainer);
  const full = useFullModel(listed);
  const model = full.spec;
  useEffect(() => { if (typeof model?.meta?.lora_family === "string") setFamily(model.meta.lora_family); }, [model]);
  const props = (model?.meta?.training_schema as { properties?: Record<string, unknown> } | undefined)?.properties;
  const local = model?.provider === "ai-toolkit";
  const canRank = local || Boolean(props?.rank || props?.lora_rank);
  const canRate = local || Boolean(props?.learning_rate);
  const selectedCharacter = characters.find((c) => c.id === source) ?? (character?.id === source ? character : undefined);
  const count = source === "images" ? images.length : selectedCharacter?.assets.length ?? 0;
  const problems = {
    trigger: trigger && !/^[A-Za-z0-9_]+$/.test(trigger.trim()) ? "Use letters, digits and underscores only." : null,
    steps: !stepsText.trim() || !Number.isInteger(steps) || steps < 100 || steps > 10000 ? "Use 100 to 10,000 steps." : null,
    rank: canRank && (!rankText.trim() || !Number.isInteger(rank) || rank < 4 || rank > 128) ? "Use a rank from 4 to 128." : null,
    rate: canRate && (!rateText.trim() || !(rate >= 0.000001 && rate <= 0.001)) ? "Use 0.000001 to 0.001." : null,
  };
  const blocker = !model
    ? trainers.length ? "Choose a trainer." : "Set up a trainer first."
    : count < 3
      ? "Choose at least 3 images."
      : !trigger.trim()
        ? "Add a trigger word."
        : Object.values(problems).some(Boolean)
          ? "Fix the marked fields."
          : null;
  const request = (): TrainRequest => ({ trainer, ...(source === "images" ? { asset_ids: images.map((a) => a.id) } : { character: source }), trigger_word: trigger.trim(), family, steps, rank: canRank ? rank : 16, learning_rate: canRate ? rate : 0.0001 });
  const submit = async (event: FormEvent) => {
    event.preventDefault(); if (busy) return; setBusy(true); setError(null);
    try {
      const body = request();
      if (estimate && JSON.stringify(estimate.body) === JSON.stringify(body)) {
        const created = await api.train(body); addJobs(created.jobs); onClose();
      } else {
        const result = await api.estimateTraining(body); setEstimate({ value: result.total_estimated_cost_usd, body });
      }
    } catch (e) { setError(errorMessage(e)); } finally { setBusy(false); }
  };
  return <Dialog title={character ? `Train ${character.name}` : "Train a LoRA"} size="wide" onClose={onClose} className="training-dialog">
    <form onSubmit={(event) => void submit(event)} onChange={() => setEstimate(null)}>
      <p className="control-help">Train a person, product or style, then use it on compatible models.</p>
      <div className="training-fields">
        <div className="training-source"><label className="field"><span>Training images</span><select value={source} onChange={(e) => setSource(e.target.value)}><option value="images">Choose library images</option>{characters.map((c) => <option key={c.id} value={c.id}>{c.name}</option>)}</select></label>
        {source === "images" && <button type="button" className="text-button" onClick={() => setPicking(true)}>Choose images ({images.length})</button>}</div>
        <p className="control-help training-limits">{count} images selected. Use 3 to 100 distinct PNG, JPEG or WebP images, at least 256 px per side. Up to 32 MiB each and 512 MiB total.</p>
        <label className="field"><span>Trainer</span><select value={trainer} onChange={(e) => setTrainer(e.target.value)}><option value="" disabled>Choose a trainer</option>{trainers.map((m) => <option key={m.id} value={m.id}>{m.provider === "ai-toolkit" ? `${familyLabel(String(m.meta?.lora_family))} / local` : m.name}</option>)}</select></label>
        {!trainers.length && <p className="control-help">Set up fal or the ai-toolkit path in <Link to="/settings">Settings &gt; Providers</Link>.</p>}
        <label className="field"><span>Base model family</span><select value={family} disabled aria-label="Base model family">{["flux1", "qwen_image", "qwen_image_2512", "wan21"].map((f) => <option key={f} value={f}>{familyLabel(f)}</option>)}</select></label>
        <label className="field"><span>Trigger word</span><input required maxLength={64} value={trigger} aria-invalid={Boolean(problems.trigger) || undefined} onChange={(e) => setTrigger(e.target.value)} placeholder="maya_person" /><small className={problems.trigger ? "field-error" : undefined}>{problems.trigger ?? "Added to generation prompts when missing."}</small></label>
        <label className="field"><span>Preset</span><select defaultValue="balanced" onChange={(e) => { const presets = { quick: [500, 16, 0.0001], balanced: [1000, 16, 0.0001], detailed: [2000, 32, 0.00005] }; const p = presets[e.target.value as keyof typeof presets]!; setSteps(String(p[0])); setRank(String(p[1])); setRate(String(p[2])); }}><option value="quick">Quick</option><option value="balanced">Balanced</option><option value="detailed">Detailed</option></select></label>
        <div className="training-parameters">
          <label className="field"><span>Steps</span><input type="text" inputMode="numeric" required value={stepsText} aria-invalid={Boolean(problems.steps) || undefined} onChange={(e) => setSteps(e.target.value)} />{problems.steps && <small className="field-error">{problems.steps}</small>}</label>
          <label className="field"><span>Rank{!canRank && " (trainer fixed)"}</span><input type="text" inputMode="numeric" disabled={!canRank} value={canRank ? rankText : "Provider default"} aria-invalid={Boolean(problems.rank) || undefined} onChange={(e) => setRank(e.target.value)} />{problems.rank && <small className="field-error">{problems.rank}</small>}</label>
          <label className="field"><span>Learning rate{!canRate && " (trainer fixed)"}</span><input type="text" inputMode="decimal" disabled={!canRate} value={canRate ? rateText : "Provider default"} aria-invalid={Boolean(problems.rate) || undefined} onChange={(e) => setRate(e.target.value)} />{problems.rate && <small className="field-error">{problems.rate}</small>}</label>
        </div>
      </div>
      {full.error && <p className="notice error" role="alert">{full.error}</p>}
      {error && <p className="notice error" role="alert">{error}</p>}
      {estimate && <p className="notice" role="status">{estimate.value === null ? "The provider does not publish a fixed estimate. Training is paid by usage." : estimate.value === 0 ? "Local training is free. It uses your GPU." : `Estimated cost: ${money(estimate.value)}. This is a paid training run.`}</p>}
      <div className="dialog-actions">{blocker && <p className="hint" role="status">{blocker}</p>}<button type="button" className="text-button" onClick={onClose}>Cancel</button><button type="submit" className="primary-button" disabled={busy || Boolean(blocker)}>{busy && <Spinner />}{estimate ? "Start training" : "Check and estimate"}</button></div>
    </form>
    {picking && <LibraryPicker kind="image" room={100} title="Choose training images" onClose={() => setPicking(false)} onPick={(assets) => { setImages(assets); setEstimate(null); }} />}
  </Dialog>;
}
