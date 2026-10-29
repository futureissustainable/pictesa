import { createPortal } from "react-dom";
import { dismissNotice, notices } from "../lib/notices";
import { useStore } from "../lib/store";
import { openDialogs } from "./dialog";
import { Icon } from "./icon";

/** Short status messages. Errors are alerts; everything else is announced politely. While a modal
 * dialog is open they render inside it: the rest of the page is inert and below it. */
export function Notices() {
  const list = useStore(notices);
  const host = useStore(openDialogs).at(-1);
  const content = (
    <div className={`notices ${host ? "in-dialog" : ""}`} aria-live="polite">
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
  return host ? createPortal(content, host) : content;
}
