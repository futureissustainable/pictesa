import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorMessage } from "../../lib/api";
import { onReconnect, subscribeEvents } from "../../lib/events";
import { money } from "../../lib/format";
import type { BudgetProgress, Spend, SpendGroup } from "../../lib/types";
import { Icon, Spinner } from "../../ui/icon";

const preciseMoney = (usd: number) => usd.toLocaleString("en-US", { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 6 });
const date = (day: string) => day.split("-").reverse().join(".");

export default function SpendSettings() {
  const [data, setData] = useState<Spend | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [daily, setDaily] = useState("");
  const [monthly, setMonthly] = useState("");
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const initialized = useRef(false);
  const reload = useCallback(() => {
    api.spend().then((value) => {
      setData(value); setError(null);
      if (!initialized.current) {
        setDaily(value.daily.cap_usd?.toString() ?? "");
        setMonthly(value.monthly.cap_usd?.toString() ?? "");
        initialized.current = true;
      }
    }).catch((reason) => setError(errorMessage(reason)));
  }, []);
  useEffect(() => {
    reload();
    let timer: ReturnType<typeof setTimeout>;
    const off = subscribeEvents({ job: () => { clearTimeout(timer); timer = setTimeout(reload, 300); } });
    const reconnect = onReconnect(reload);
    return () => { off(); reconnect(); clearTimeout(timer); };
  }, [reload]);

  if (!data) return error ? <div className="notice error" role="alert">{error}<button type="button" className="text-button" onClick={reload}>Try again</button></div> : <div className="page-loading" role="status"><Spinner />Loading spend</div>;
  return <div className="settings-list">
    <section className="settings-block" aria-labelledby="spend-title">
      <div className="collection-heading"><h2 id="spend-title">Spend</h2><button type="button" className="icon-button" aria-label="Refresh spend" onClick={reload}><Icon name="refresh" /></button></div>
      <div className="spend-totals">{[["Today", data.today_usd], ["7 days", data.seven_days_usd], ["30 days", data.thirty_days_usd]].map(([label, value]) => <div key={label}><p className="hint">{label}</p><p className="balance-value">{money(Number(value))}</p></div>)}</div>
      <p>Includes queued runs and estimates when a provider does not report a final charge. Local runs cost $0. Calendar days and months use {data.timezone}.</p>
      {data.unknown_runs > 0 && <p className="notice">{data.unknown_runs} runs have no known price and are excluded from totals.</p>}
      {data.estimated_runs > 0 && <p className="hint">{data.estimated_runs} runs use estimated costs.</p>}
    </section>
    <section className="settings-block" aria-labelledby="daily-spend-title"><h2 id="daily-spend-title">Daily spend</h2><SpendChart days={data.days} /></section>
    <section className="settings-block" aria-labelledby="budgets-title">
      <h2 id="budgets-title">Budgets</h2><p>A run that would exceed a cap stops before it is sent. Leave a cap empty for no limit. Zero blocks paid runs.</p>
      <form className="library-form" onSubmit={async (event) => {
        event.preventDefault(); setBusy(true); setSaved(false); setError(null);
        try { setData(await api.setBudgets(daily.trim() ? Number(daily) : null, monthly.trim() ? Number(monthly) : null)); setSaved(true); }
        catch (reason) { setError(errorMessage(reason)); } finally { setBusy(false); }
      }}>
        <div className="spend-caps">
          <label className="field"><span>Daily cap (USD)</span><input type="number" min="0" max="1000000" step="0.000001" placeholder="No limit" value={daily} onChange={(event) => { setDaily(event.target.value); setSaved(false); }} /></label>
          <label className="field"><span>Monthly cap (USD)</span><input type="number" min="0" max="1000000" step="0.000001" placeholder="No limit" value={monthly} onChange={(event) => { setMonthly(event.target.value); setSaved(false); }} /></label>
        </div>
        <Budget label="Daily" value={data.daily} /><Budget label="Monthly" value={data.monthly} />
        {error && <p role="alert" className="notice error">{error}</p>}
        <div className="dialog-actions">{saved && <span role="status" className="hint">Budgets saved.</span>}<button className="primary-button" disabled={busy}>Save budgets</button></div>
      </form>
    </section>
    <div className="spend-breakdowns"><Breakdown title="By model" rows={data.models} /><Breakdown title="By provider" rows={data.providers} /></div>
  </div>;
}

function Budget({ label, value }: { label: string; value: BudgetProgress }) {
  return <div className="budget-progress"><div className="field-top"><span>{label}</span><span>{preciseMoney(value.spent_usd)}{value.cap_usd !== null ? ` / ${preciseMoney(value.cap_usd)}` : " spent, no cap"}</span></div>
    {value.cap_usd !== null && <><progress aria-label={`${label} budget used`} value={Math.min(value.spent_usd, value.cap_usd || 1)} max={value.cap_usd || 1} /><p className="hint">{preciseMoney(value.remaining_usd ?? 0)} left</p></>}
  </div>;
}

function SpendChart({ days }: { days: SpendGroup[] }) {
  const [active, setActive] = useState<SpendGroup | null>(null);
  const max = Math.max(...days.map((day) => day.usd), 0.01);
  const displayed = active ?? days.at(-1);
  return <div className="spend-chart">
    <p className="hint spend-readout" aria-live="polite">{displayed && `${date(displayed.name)}: ${preciseMoney(displayed.usd)}`}</p>
    <div className="spend-bars" role="group" aria-label="Spend per day over 30 days">
      {days.map((day) => <button type="button" key={day.name} aria-label={`${date(day.name)}: ${preciseMoney(day.usd)}`} title={`${date(day.name)}: ${preciseMoney(day.usd)}`} onFocus={() => setActive(day)} onBlur={() => setActive(null)} onMouseEnter={() => setActive(day)} onMouseLeave={() => setActive(null)} style={{ height: `${Math.max(1, day.usd / max * 100)}%` }} />)}
    </div>
    <div className="spend-axis">{[days[0], days[Math.floor(days.length / 2)], days.at(-1)].map((day) => day && <span key={day.name}>{date(day.name).slice(0, 5)}</span>)}</div>
  </div>;
}

function Breakdown({ title, rows }: { title: string; rows: SpendGroup[] }) {
  return <section className="settings-block"><h2>{title}</h2><p className="hint">Last 30 days</p>{rows.length ? <ul className="token-list">{rows.map((row) => <li key={row.name} className="connection-row spend-row"><span>{row.name}</span><span>{preciseMoney(row.usd)}</span></li>)}</ul> : <p className="hint">No runs yet.</p>}</section>;
}
