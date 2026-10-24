import { useState } from "react";
import { errorMessage } from "../../lib/api";
import { formatDate } from "../../lib/format";
import { notifyError } from "../../lib/notices";
import { logout, useSession } from "../../lib/session";
import { Icon, Spinner } from "../../ui/icon";

export default function Account() {
  const session = useSession();
  const [busy, setBusy] = useState(false);
  const user = session.status === "ready" ? session.me.user : null;
  if (!user) return null;
  return (
    <div className="settings-list">
      <section className="settings-block" aria-labelledby="account-title">
        <h2 id="account-title">{user.name || user.email}</h2>
        <dl className="about-list">
          <div>
            <dt>Email</dt>
            <dd>{user.email}</dd>
          </div>
          <div>
            <dt>Role</dt>
            <dd>{user.role === "admin" ? "Admin" : "Member"}</dd>
          </div>
          <div>
            <dt>Joined</dt>
            <dd>{formatDate(user.created_at)}</dd>
          </div>
        </dl>
        <button
          type="button"
          className="text-button"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            try {
              await logout();
            } catch (error) {
              notifyError(errorMessage(error));
              setBusy(false);
            }
          }}
        >
          {busy ? <Spinner /> : <Icon name="logout" />}
          Log out
        </button>
      </section>
    </div>
  );
}
