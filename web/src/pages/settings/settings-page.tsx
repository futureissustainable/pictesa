import { lazy, Suspense, useEffect, useRef } from "react";
import { useBilling } from "../../lib/catalog";
import { Link, useLocation } from "../../lib/router";
import { useSession } from "../../lib/session";
import { Icon, Spinner, type IconName } from "../../ui/icon";
import { Providers } from "./providers";

const Agents = lazy(() => import("./agents"));
const About = lazy(() => import("./about"));
const Account = lazy(() => import("./account"));
const SpendSettings = lazy(() => import("./spend"));
const BillingSettings = lazy(() => import("./billing"));

type Tab = "providers" | "agents" | "billing" | "spend" | "about" | "account";

export default function SettingsPage() {
  const url = useLocation();
  const session = useSession();
  const hosted = session.status === "ready" && session.me.mode === "hosted";
  const billing = useBilling(hosted).data?.enabled ?? false;
  const path = url.pathname.replace(/^\/settings\/?/, "");
  const tab: Tab = path === "spend" || path === "agents" || path === "about" || (path === "account" && hosted) || (path === "billing" && hosted) ? path : "providers";
  const tabs: { id: Tab; label: string; icon: IconName; to: string }[] = [
    { id: "providers", label: "Providers", icon: "key", to: "/settings" },
    { id: "spend", label: "Spend", icon: "account_balance_wallet", to: "/settings/spend" },
    { id: "agents", label: "Agents", icon: "smart_toy", to: "/settings/agents" },
    ...(hosted && billing ? [{ id: "billing" as const, label: "Billing", icon: "account_balance_wallet" as const, to: "/settings/billing" }] : []),
    { id: "about", label: "About", icon: "info", to: "/settings/about" },
    ...(hosted ? [{ id: "account" as const, label: "Account", icon: "person" as const, to: "/settings/account" }] : []),
  ];

  // On a phone the tabs scroll sideways: keep the current one in view.
  const tabsRef = useRef<HTMLElement>(null);
  useEffect(() => {
    const nav = tabsRef.current;
    const active = nav?.querySelector<HTMLElement>('[aria-current="page"]');
    if (!nav || !active) return;
    const box = nav.getBoundingClientRect();
    const item = active.getBoundingClientRect();
    if (item.left < box.left || item.right > box.right) nav.scrollLeft += item.left - box.left - 16;
  }, [tab, tabs.length]);

  return (
    <div className="workspace settings">
      <div className="intro">
        <div>
          <h1>Settings.</h1>
          <p>Providers, agents and this install.</p>
        </div>
      </div>
      <nav className="tabs" aria-label="Settings sections" ref={tabsRef}>
        {tabs.map((item) => (
          <Link key={item.id} to={item.to} className={`filter-button ${tab === item.id ? "active" : ""}`} aria-current={tab === item.id ? "page" : undefined}>
            <Icon name={item.icon} />
            {item.label}
          </Link>
        ))}
      </nav>
      <Suspense
        fallback={
          <div className="page-loading" role="status">
            <Spinner />
            Loading
          </div>
        }
      >
        {tab === "spend" ? <SpendSettings /> : tab === "agents" ? <Agents /> : tab === "about" ? <About /> : tab === "account" ? <Account /> : tab === "billing" ? <BillingSettings /> : <Providers />}
      </Suspense>
    </div>
  );
}
