import { useLoras, loraFamily } from "../../lib/loras";
import { setLora, useComposer } from "../../lib/composer";
import type { ModelSpec } from "../../lib/types";
import { Link } from "../../lib/router";

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
  return <div className="composer-loras">
    <label className="field"><span>LoRA</span><select value={selection?.asset_id ?? ""} onChange={(e) => setLora(model.id, e.target.value ? { asset_id: e.target.value, strength: 1 } : null)}><option value="">No LoRA</option>{data.map((l) => <option key={l.asset.id} value={l.asset.id}>{l.trigger_word}</option>)}</select></label>
    {selected && <label className="field"><span>Strength {selection!.strength.toFixed(2)}</span><input type="range" min={0} max={2} step={0.05} value={selection!.strength} onChange={(e) => setLora(model.id, { asset_id: selected.asset.id, strength: Number(e.target.value) })} /><small>Trigger: {selected.trigger_word}. Added to your prompt when missing.</small></label>}
    {offered && <button type="button" className="text-button" onClick={() => setLora(model.id, { asset_id: offered.asset.id, strength: 1 })}>Use this character’s trained LoRA</button>}
    {!data.length && !error && <Link className="text-button" to="/loras">Train a LoRA</Link>}
    {error && <p className="notice error" role="alert">{error}</p>}
  </div>;
}
