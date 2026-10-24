import { dismissNotice, notices } from "../lib/notices";
import { useStore } from "../lib/store";
import { Icon } from "./icon";

/** Short status messages. Errors are alerts; everything else is announced politely. */
export function Notices() {
  const list = useStore(notices);
  return (
    <div className="notices" aria-live="polite">
      {list.map((notice) => (
        <div key={notice.id} className={`notice notice-toast ${notice.tone === "error" ? "error" : ""}`} role={notice.tone === "error" ? "alert" : "status"}>
          <p>{notice.text}</p>
          {notice.action && (
            <button
              type="button"
              className="text-button notice-action"
              onClick={() => {
                notice.action?.run();
                dismissNotice(notice.id);
              }}
            >
              {notice.action.label}
            </button>
          )}
          <button type="button" className="icon-button" aria-label="Dismiss message" onClick={() => dismissNotice(notice.id)}>
            <Icon name="close" />
          </button>
        </div>
      ))}
    </div>
  );
}
