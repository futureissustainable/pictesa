import { useLoras, loraFamily } from "../../lib/loras";
import { setLora, useComposer } from "../../lib/composer";
import type { ModelSpec } from "../../lib/types";
import { Link } from "../../lib/router";
import { Choice, Control } from "../../ui/control";

/** The LoRA setting on the control line, for models that take one. */
export function LoraPicker({ model, characterId }: { model: ModelSpec; characterId: string | null }) {
  const family = loraFamily(model);
  if (!family) return null;
  return <Picker model={model} family={family} characterId={characterId} />;
}
function Picker({ model, family, characterId }: { model: ModelSpec; family: string; characterId: string | null }) {
  const { data, error } = useLoras(family);
  const draft = useComposer();
  const selection = draft.loraByModel?.[model.id];
  const selected = data.find((l) => l.asset.id === selection?.asset_id);
  const offered = characterId && selected?.character_id !== characterId && data.find((l) => l.character_id === characterId);
  return (
    <Control label="LoRA" icon="model_training" value={selected ? selected.trigger_word : "No LoRA"} className={selected ? "has-changes" : ""} invalid={Boolean(error)}>
      <div className="control-list">
        <Choice selected={!selected} keepOpen onPick={() => setLora(model.id, null)}>
          No LoRA
        </Choice>
        {data.map((l) => (
          <Choice key={l.asset.id} selected={l.asset.id === selected?.asset.id} keepOpen onPick={() => setLora(model.id, { asset_id: l.asset.id, strength: selection?.strength ?? 1 })}>
            {l.trigger_word}
          </Choice>
        ))}
      </div>
      {selected && (
        <label className="field control-field">
          <span>Strength {selection!.strength.toFixed(2)}</span>
          <input type="range" min={0} max={2} step={0.05} value={selection!.strength} onChange={(e) => setLora(model.id, { asset_id: selected.asset.id, strength: Number(e.target.value) })} />
          <small>Trigger: {selected.trigger_word}. Added to your prompt when missing.</small>
        </label>
      )}
      {offered && (
        <Choice onPick={() => setLora(model.id, { asset_id: offered.asset.id, strength: 1 })}>Use this character’s trained LoRA</Choice>
      )}
      {error && (
        <p className="control-help" role="alert">
          {error}
        </p>
      )}
      <p className="control-help">
        <Link to="/loras">{data.length ? "Manage LoRAs" : "Train a LoRA"}</Link>
      </p>
    </Control>
  );
}
