import { Component, lazy, Suspense, useEffect, type ErrorInfo, type ReactNode } from "react";
import { useBilling } from "./lib/catalog";
import { connection, startEvents, stopEvents } from "./lib/events";
import { money } from "./lib/format";
import { isActive, jobs, loadActiveJobs } from "./lib/jobs";
import { loadActiveRuns } from "./lib/recipes";
import { Link, navigate, useLocation } from "./lib/router";
import { loadSession, useSession } from "./lib/session";
import { useStore } from "./lib/store";
import type { Me } from "./lib/types";
import { Brand, PixelMark } from "./ui/brand";
import { Icon, Spinner, type IconName } from "./ui/icon";
import { Notices } from "./ui/notices";

const CreatePage = lazy(() => import("./pages/create/create-page"));
const LibraryPage = lazy(() => import("./pages/library/library-page"));
const RecipesPage = lazy(() => import("./pages/recipes/recipes-page"));
const CharactersPage = lazy(() => import("./pages/characters/characters-page"));
const SettingsPage = lazy(() => import("./pages/settings/settings-page"));
const AuthPage = lazy(() => import("./pages/auth/auth-page"));
const TokenPage = lazy(() => import("./pages/auth/auth-page").then((module) => ({ default: module.TokenPage })));
const Viewer = lazy(() => import("./pages/viewer/viewer"));
const EditPage = lazy(() => import("./pages/edit/edit-page"));

type Section = "create" | "library" | "favorites" | "uploads" | "collections" | "characters" | "trash" | "settings" | "none";

interface NavItem {
  id: Section;
  label: string;
  icon: IconName;
  to: string;
}

const primary: NavItem[] = [
  { id: "create", label: "Create", icon: "add_circle", to: "/" },
  { id: "library", label: "Library", icon: "grid_view", to: "/library" },
];
const collections: NavItem[] = [
  { id: "collections", label: "Collections", icon: "folder_open", to: "/library?collections=true" },
  { id: "favorites", label: "Favorites", icon: "favorite", to: "/library?favorite=true" },
  { id: "uploads", label: "Uploads", icon: "upload", to: "/library?source=upload" },
  { id: "characters", label: "Characters", icon: "face", to: "/characters" },
  { id: "trash", label: "Trash", icon: "delete", to: "/library?trash=true" },
];
const settingsItem: NavItem = { id: "settings", label: "Settings", icon: "tune", to: "/settings" };
const mobileItems = [primary[0]!, primary[1]!, collections[1]!, settingsItem];

function sectionOf(url: URL): Section {
  if (url.pathname === "/" || url.pathname === "/recipes" || url.pathname === "/edit") return "create";
  if (url.pathname.startsWith("/settings")) return "settings";
  if (url.pathname === "/characters") return "characters";
  if (url.pathname === "/library") {
    if (url.searchParams.has("collection") || url.searchParams.has("collections")) return "collections";
    if (url.searchParams.get("trash") === "true") return "trash";
    if (url.searchParams.get("favorite") === "true") return "favorites";
    if (url.searchParams.get("source") === "upload") return "uploads";
    return "library";
  }
  return "none";
}

const titles: Record<Section, string> = {
  create: "Create",
  library: "Library",
  favorites: "Favorites",
  collections: "Collections",
  uploads: "Uploads",
  characters: "Characters",
  trash: "Trash",
  settings: "Settings",
  none: "Not found",
};

export function App() {
  const state = useSession();
  useEffect(() => {
    void loadSession();
  }, []);

  if (state.status === "loading") return <Splash />;
  if (state.status === "offline") return <Offline message={state.message} />;
  if (state.me.needs_token) {
    return (
      <Suspense fallback={<Splash />}>
        <TokenPage />
      </Suspense>
    );
  }
  if (state.me.mode === "hosted" && !state.me.user) {
    return (
      <Suspense fallback={<Splash />}>
        <AuthPage />
      </Suspense>
    );
  }
  return <Shell me={state.me} />;
}

function Splash() {
  return (
    <div className="splash" role="status" aria-label="Loading Pictesa">
      <PixelMark />
    </div>
  );
}

function Offline({ message }: { message: string }) {
  useEffect(() => {
    const timer = setInterval(() => void loadSession(), 4000);
    return () => clearInterval(timer);
  }, []);
  return (
    <main className="splash splash-message">
      <PixelMark />
      <h1>Pictesa is offline.</h1>
      <p>{message} This page reconnects on its own.</p>
      <button type="button" className="primary-button" onClick={() => void loadSession()}>
        Try again
        <Icon name="refresh" />
      </button>
    </main>
  );
}

function Shell({ me }: { me: Me }) {
  const url = useLocation();
  const section = sectionOf(url);
  const running = useStore(jobs).filter(isActive).length;
  const assetId = url.searchParams.get("asset");

  useEffect(() => {
    startEvents();
    void loadActiveJobs();
    void loadActiveRuns();
    return () => stopEvents();
  }, []);

  useEffect(() => {
    if (url.pathname === "/login" || url.pathname === "/signup") navigate("/", { replace: true });
  }, [url.pathname]);

  const recipes = url.pathname === "/recipes";
  const editing = url.pathname === "/edit";
  const pageTitle = recipes ? "Recipes" : editing ? "Edit" : titles[section];
  useEffect(() => {
    document.title = `${pageTitle} | Pictesa`;
  }, [pageTitle]);

  const navButton = (item: NavItem) => (
    <Link
      key={item.id}
      to={item.to}
      className={`nav-button ${section === item.id ? "active" : ""}`}
      aria-current={section === item.id ? "page" : undefined}
      title={item.label}
    >
      <Icon name={item.icon} filled={section === item.id && item.id === "favorites"} />
      <span>{item.label}</span>
      {item.id === "create" && running > 0 && (
        <span className="nav-count" aria-label={`${running} running`}>
          {running}
        </span>
      )}
    </Link>
  );

  return (
    <div className={`app ${section === "create" ? "creating" : ""}`}>
      <a className="skip-link" href="#workspace">
        Skip to workspace
      </a>
      <aside className="sidebar" aria-label="Main navigation">
        <Brand />
        <nav className="nav-group" aria-label="Studio">
          {primary.map(navButton)}
        </nav>
        <nav className="nav-group" aria-label="Collections">
          {collections.map(navButton)}
        </nav>
        <div className="sidebar-bottom">{navButton(settingsItem)}</div>
      </aside>
      <main className="main" id="workspace">
        <Topbar title={pageTitle} me={me} />
        <ErrorBoundary key={section}>
          <Suspense
            fallback={
              <div className="page-loading" role="status">
                <Spinner />
                Loading
              </div>
            }
          >
            {recipes ? (
              <RecipesPage />
            ) : editing ? (
              <EditPage />
            ) : section === "create" ? (
              <CreatePage />
            ) : section === "settings" ? (
              <SettingsPage />
            ) : section === "characters" ? (
              <CharactersPage />
            ) : section === "none" ? (
              <NotFound />
            ) : (
              <LibraryPage />
            )}
          </Suspense>
        </ErrorBoundary>
      </main>
      <nav className="mobile-nav" aria-label="Mobile navigation">
        {mobileItems.map((item) => (
          <Link
            key={item.id}
            to={item.to}
            className={section === item.id ? "active" : ""}
            aria-current={section === item.id ? "page" : undefined}
          >
            <Icon name={item.icon} filled={section === item.id && item.id === "favorites"} />
            {item.label}
            {item.id === "create" && running > 0 && <span className="nav-dot" aria-label={`${running} running`} />}
          </Link>
        ))}
      </nav>
      <Notices />
      {assetId && (
        <Suspense fallback={null}>
          <Viewer id={assetId} />
        </Suspense>
      )}
    </div>
  );
}

function Topbar({ title, me }: { title: string; me: Me }) {
  const state = useStore(connection);
  const billing = useBilling(me.mode === "hosted" && Boolean(me.user)).data;
  return (
    <header className="topbar">
      <Brand className="mobile-brand" />
      <p className="breadcrumb">
        <strong>{title}</strong>
      </p>
      <div className="topbar-right">
        {state === "reconnecting" && (
          <span className="connection" role="status">
            <Icon name="cloud_off" />
            Reconnecting
          </span>
        )}
        {me.mode === "hosted" && me.user && (
          <Link
            to={billing?.enabled ? "/settings/billing" : "/settings/account"}
            className="account-button"
            aria-label={billing?.enabled ? `Balance ${money(billing.balance_usd)}, billing settings` : "Account settings"}
          >
            <span className="account-label">{billing?.enabled ? money(billing.balance_usd) : me.user.name || me.user.email}</span>
            <span className="avatar">{(me.user.name || me.user.email).slice(0, 1).toUpperCase()}</span>
          </Link>
        )}
      </div>
    </header>
  );
}

function NotFound() {
  return (
    <div className="workspace">
      <div className="empty">
        <Icon name="search_off" />
        <h2>This page does not exist.</h2>
        <p>The link may be old. Your work is still in the library.</p>
        <Link className="primary-button" to="/">
          Go to Create
          <Icon name="arrow_forward" />
        </Link>
      </div>
    </div>
  );
}

class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  override state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  override componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(error, info.componentStack);
  }
  override render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="workspace">
        <div className="empty">
          <Icon name="error" />
          <h2>This view stopped working.</h2>
          <p>{this.state.error.message}</p>
          <button type="button" className="primary-button" onClick={() => location.reload()}>
            Reload
            <Icon name="refresh" />
          </button>
        </div>
      </div>
    );
  }
}
