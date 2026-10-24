import { useEffect, useRef, useState } from "react";
import { api, errorMessage } from "../../lib/api";
import { loadBilling, useBilling } from "../../lib/catalog";
import { formatDateTime, money } from "../../lib/format";
import { useLocation } from "../../lib/router";
import { Icon, Spinner } from "../../ui/icon";

const requestId = () => `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;

/** Hosted wallet: balance, top-ups through Stripe Checkout and the ledger. */
export default function BillingSettings() {
  const { data, error } = useBilling(true);
  const url = useLocation();
  const [busy, setBusy] = useState<number | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  // One request id per amount and visit, so a double click cannot start two checkouts.
  const ids = useRef<Record<number, string>>({});
  const returned = url.searchParams.get("checkout");

  useEffect(() => {
    if (returned) void loadBilling(true);
  }, [returned]);

  if (!data) {
    return error ? (
      <p className="notice error" role="alert">
        {error}
      </p>
    ) : (
      <div className="page-loading" role="status">
        <Spinner />
        Loading balance
      </div>
    );
  }

  if (!data.enabled) return <p className="hint">Billing is off on this server. Runs use the server's own provider keys.</p>;

  const topUp = async (amount: number) => {
    setBusy(amount);
    setFailure(null);
    try {
      ids.current[amount] ??= requestId();
      const { url: checkout } = await api.checkout(Math.round(amount * 100), ids.current[amount]);
      window.location.assign(checkout);
    } catch (reason) {
      setFailure(errorMessage(reason));
      setBusy(null);
    }
  };

  return (
    <div className="settings-list">
      <section className="settings-block billing-head" aria-labelledby="balance-title">
        <h2 id="balance-title">Balance</h2>
        <p className="balance-value">{money(data.balance_usd)}</p>
        <p>
          Each run is paid from this balance at the price shown before you start
          {data.markup > 1 ? `, which includes a ${Math.round((data.markup - 1) * 100)}% service markup` : ""}.
        </p>
        {data.debt_usd > 0 && <p className="notice error">Unpaid: {money(data.debt_usd)}. It is taken from your next top-up.</p>}
        {data.locked && (
          <p className="notice error" role="alert">
            New runs are paused until the balance covers them.
          </p>
        )}
        {returned === "success" && (
          <p className="notice" role="status">
            Payment received. The balance updates as soon as Stripe confirms it.
          </p>
        )}
        {data.topups_available ? (
          <div className="funding-options">
            {data.topups_usd.map((amount) => (
              <button key={amount} type="button" className="primary-button" disabled={busy !== null} onClick={() => void topUp(amount)}>
                {busy === amount ? <Spinner /> : <Icon name="add" />}
                Add {money(amount)}
              </button>
            ))}
          </div>
        ) : (
          <p className="hint">Top-ups are not available on this server yet.</p>
        )}
        {failure && (
          <p className="notice error" role="alert">
            {failure}
          </p>
        )}
        {data.topups_available && <p className="hint">Secure checkout with Stripe.</p>}
      </section>
      <section className="settings-block" aria-labelledby="ledger-title">
        <h2 id="ledger-title">Activity</h2>
        {data.ledger.length ? (
          <ul className="token-list">
            {data.ledger.map((entry) => (
              <li key={entry.id} className="connection-row">
                <span>
                  {entry.reason}
                  <small className="ledger-date">{formatDateTime(entry.created_at)}</small>
                </span>
                <span className={`ledger-amount ${entry.amount_usd > 0 ? "credit" : ""}`}>
                  {entry.amount_usd > 0 ? "+" : "−"}
                  {money(Math.abs(entry.amount_usd))}
                </span>
              </li>
            ))}
          </ul>
        ) : (
          <p className="hint">No payments or runs yet.</p>
        )}
      </section>
    </div>
  );
}
