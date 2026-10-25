import { useModels } from "../../lib/catalog";
import { setMode } from "../../lib/composer";
import { kindIcon, kindLabel } from "../../lib/format";
import { navigate } from "../../lib/router";
import type { MediaKind } from "../../lib/types";
import { Icon } from "../../ui/icon";

/** Image | Video (| Audio when the server has audio models) | Apps | Edit, shared by Create, Apps and Edit. */
export function ModeSwitch({ active }: { active: MediaKind | "apps" | "edit" }) {
  const audio = useModels("audio");
  const kinds: MediaKind[] = audio.data?.length ? ["image", "video", "audio"] : ["image", "video"];
  return (
    <div className="segmented mode-switch" role="group" aria-label="What to create">
      {kinds.map((kind) => (
        <button
          key={kind}
          type="button"
          aria-pressed={active === kind}
          onClick={() => {
            setMode(kind);
            if (active === "apps" || active === "edit") navigate("/");
          }}
        >
          <Icon name={kindIcon[kind]} filled={active === kind} />
          <span>{kindLabel[kind]}</span>
        </button>
      ))}
      <button type="button" aria-pressed={active === "apps"} onClick={() => active !== "apps" && navigate("/apps")}>
        <Icon name="apps" filled={active === "apps"} />
        <span>Apps</span>
      </button>
      <button type="button" aria-pressed={active === "edit"} onClick={() => active !== "edit" && navigate("/edit")}>
        <Icon name="edit" filled={active === "edit"} />
        <span>Edit</span>
      </button>
    </div>
  );
}
