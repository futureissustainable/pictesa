import { useModels } from "../../lib/catalog";
import { setMode } from "../../lib/composer";
import { kindIcon, kindLabel } from "../../lib/format";
import { navigate } from "../../lib/router";
import type { MediaKind } from "../../lib/types";
import { Icon } from "../../ui/icon";

/** Image | Video (| Audio when the server has audio models) | Recipes | Edit, shared by Create, Recipes and Edit. */
export function ModeSwitch({ active }: { active: MediaKind | "recipes" | "edit" }) {
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
            if (active === "recipes" || active === "edit") navigate("/");
          }}
        >
          <Icon name={kindIcon[kind]} filled={active === kind} />
          <span>{kindLabel[kind]}</span>
        </button>
      ))}
      <button type="button" aria-pressed={active === "recipes"} onClick={() => active !== "recipes" && navigate("/recipes")}>
        <Icon name="conversion_path" filled={active === "recipes"} />
        <span>Recipes</span>
      </button>
      <button type="button" aria-pressed={active === "edit"} onClick={() => active !== "edit" && navigate("/edit")}>
        <Icon name="edit" filled={active === "edit"} />
        <span>Edit</span>
      </button>
    </div>
  );
}
