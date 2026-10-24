import { Fragment, useMemo } from "react";
import { usePresets } from "../../lib/catalog";
import { setPreset } from "../../lib/composer";
import type { MediaKind, Preset } from "../../lib/types";
import { Icon, type IconName } from "../../ui/icon";

const categoryIcons: Record<string, IconName> = {
  camera: "videocam",
  motion: "videocam",
  style: "palette",
  lighting: "light_mode",
  light: "light_mode",
};

const titleCase = (text: string) => text.charAt(0).toUpperCase() + text.slice(1).replaceAll("_", " ");

/** Camera moves and styles from GET /api/presets. One preset at a time; pick it again to clear. */
export function PresetsStrip({ mode, selected }: { mode: MediaKind; selected: string | null }) {
  const { data } = usePresets(mode);
  const groups = useMemo(() => {
    const map = new Map<string, Preset[]>();
    for (const preset of data ?? []) {
      if (preset.output !== mode) continue;
      map.set(preset.category, [...(map.get(preset.category) ?? []), preset]);
    }
    return [...map];
  }, [data, mode]);

  if (!groups.length) return null;
  return (
    <div className="presets" role="group" aria-label="Presets">
      {groups.map(([category, presets]) => (
        <Fragment key={category}>
          <span className="presets-category">
            <Icon name={categoryIcons[category] ?? "auto_awesome"} />
            {titleCase(category)}
          </span>
          {presets.map((preset) => (
            <button
              key={preset.id}
              type="button"
              className="chip"
              aria-pressed={selected === preset.id}
              title={preset.description ?? undefined}
              onClick={() => setPreset(mode, selected === preset.id ? null : preset.id)}
            >
              {preset.name}
            </button>
          ))}
        </Fragment>
      ))}
    </div>
  );
}
