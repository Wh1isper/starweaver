import { useDesktopStatus } from "./useDesktopStatus";
import "./styles.css";

function StatusPanel() {
  const state = useDesktopStatus();

  if (state.kind === "loading") {
    return <p className="status-message">Checking the local desktop service...</p>;
  }

  if (state.kind === "error") {
    return (
      <div className="notice notice-error" role="alert">
        The desktop backend is unavailable. Restart Starweaver and try again.
      </div>
    );
  }

  const { status } = state;
  return (
    <dl className="status-grid" aria-label="Desktop status">
      <div>
        <dt>Desktop shell</dt>
        <dd>v{status.appVersion}</dd>
      </div>
      <div>
        <dt>Platform</dt>
        <dd>
          {status.platform} / {status.architecture}
        </dd>
      </div>
      <div>
        <dt>Application instance</dt>
        <dd>Primary · generation {status.launchGeneration}</dd>
      </div>
      <div>
        <dt>Managed runtime</dt>
        <dd className="status-muted">Not configured</dd>
      </div>
    </dl>
  );
}

export default function App() {
  return (
    <main className="app-shell">
      <section className="hero" aria-labelledby="page-title">
        <p className="eyebrow">Starweaver Desktop</p>
        <h1 id="page-title">Local agent workspace</h1>
        <p className="hero-copy">
          The native shell is ready. Runtime connectivity will be enabled after the versioned RPC
          launch contract is available.
        </p>
      </section>

      <section className="panel" aria-labelledby="status-title">
        <div className="panel-heading">
          <div>
            <p className="section-label">Foundation</p>
            <h2 id="status-title">System status</h2>
          </div>
          <span className="platform-badge">Local only</span>
        </div>
        <StatusPanel />
      </section>

      <footer>
        Renderer access is restricted to typed application commands. No runtime, storage, shell, or
        OAuth authority is exposed here.
      </footer>
    </main>
  );
}
