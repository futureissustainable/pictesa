import { lazy, Suspense } from "react";
import { Link, useLocation } from "../../lib/router";
import { useSession } from "../../lib/session";
import { Icon, Spinner, type IconName } from "../../ui/icon";
import { Providers } from "./providers";

const Agents = lazy(() => import("./agents"));
const About = lazy(() => import("./about"));
const Account = lazy(() => import("./account"));

type Tab = "providers" | "agents" | "about" | "account";

export default function SettingsPage() {
  const url = useLocation();
  const session = useSession();
  const hosted = session.status === "ready" && session.me.mode === "hosted";
  const path = url.pathname.replace(/^\/settings\/?/, "");
  const tab: Tab = path === "agents" || path === "about" || (path === "account" && hosted) ? path : "providers";
  const tabs: { id: Tab; label: string; icon: IconName; to: string }[] = [
    { id: "providers", label: "Providers", icon: "key", to: "/settings" },
    { id: "agents", label: "Agents", icon: "smart_toy", to: "/settings/agents" },
    { id: "about", label: "About", icon: "info", to: "/settings/about" },
    ...(hosted ? [{ id: "account" as const, label: "Account", icon: "person" as const, to: "/settings/account" }] : []),
  ];

  return (
    <div className="workspace settings">
      <div className="intro">
        <div>
          <h1>Settings.</h1>
          <p>Providers, agents and this install.</p>
        </div>
      </div>
      <nav className="tabs" aria-label="Settings sections">
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
        {tab === "agents" ? <Agents /> : tab === "about" ? <About /> : tab === "account" ? <Account /> : <Providers />}
      </Suspense>
    </div>
  );
}
