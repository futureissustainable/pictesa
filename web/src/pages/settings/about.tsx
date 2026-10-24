import { useEffect } from "react";
import { refreshHealth, useSession } from "../../lib/session";
import { serverOrigin } from "../../lib/snippets";
import { PixelMark } from "../../ui/brand";
import { CopyButton } from "../../ui/code";
import { Icon, Spinner } from "../../ui/icon";

export const SOURCE_URL = "https://github.com/futureissustainable/pictesa";

export default function About() {
  const session = useSession();
  const health = session.status === "ready" ? session.health : null;
  const mode = session.status === "ready" ? session.me.mode : null;

  useEffect(() => {
    void refreshHealth();
  }, []);

  return (
    <div className="settings-list">
      <section className="settings-block about-head">
        <PixelMark />
        <div>
          <h2>Pictesa {health?.version ? <span className="version">{health.version}</span> : null}</h2>
          <p>An open studio for images and video. One binary serves this app, the API and the agent tools.</p>
        </div>
      </section>
      <section className="settings-block" aria-label="This install">
        {!health ? (
          <p className="hint" role="status">
            <Spinner /> Reading server status
          </p>
        ) : (
          <dl className="about-list">
            <div>
              <dt>
                <Icon name="dns" />
                Server
              </dt>
              <dd>
                {serverOrigin()}
                <span className="hint">{mode === "hosted" ? "Hosted mode, with accounts" : "Local mode"}</span>
              </dd>
            </div>
            <div>
              <dt>
                <Icon name="folder_open" />
                Data folder
              </dt>
              <dd className="about-path">
                <code>{health.data_dir}</code>
                <CopyButton text={health.data_dir} label="Copy data folder path" />
              </dd>
            </div>
            <div>
              <dt>
                <Icon name="movie" />
                ffmpeg
              </dt>
              <dd>
                <span className={`status ${health.ffmpeg ? "" : "off"}`}>{health.ffmpeg ? "Installed" : "Not found"}</span>
                {!health.ffmpeg && <span className="hint">Video thumbnails and durations need ffmpeg on the server's PATH.</span>}
              </dd>
            </div>
          </dl>
        )}
      </section>
      <section className="settings-block" aria-labelledby="licence-title">
        <h2 id="licence-title">Licence</h2>
        <p>
          Pictesa is free software under the GNU Affero General Public License, version 3. You can use, study, share and change it. If you run a changed version for others over a network, offer them its source.
        </p>
        <div className="detail-actions">
          <a className="text-button" href={SOURCE_URL} target="_blank" rel="noreferrer">
            <Icon name="code" />
            Source code
          </a>
          <a className="text-button" href={`${SOURCE_URL}/blob/main/LICENSE`} target="_blank" rel="noreferrer">
            <Icon name="license" />
            AGPL-3.0 text
          </a>
        </div>
      </section>
    </div>
  );
}
