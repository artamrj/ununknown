import type { Setup } from "@/api/types";
import { Icon } from "../Icons";
import { providerName } from "../trackUtils";
import { Secret } from "./TrackInspector";

export function SettingsDrawer({
  setup,
  setSetup,
  keys,
  setKeys,
  saving,
  onSave,
  onClose,
}: {
  setup: Setup;
  setSetup: (setup: Setup) => void;
  keys: Record<string, string>;
  setKeys: (keys: Record<string, string>) => void;
  saving: boolean;
  onSave: () => Promise<void>;
  onClose: () => void;
}) {
  return (
    <div className="drawer-layer" role="presentation">
      <button className="drawer-backdrop" onClick={onClose} aria-label="Close settings" />
      <aside
        className="settings-drawer"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
      >
        <header>
          <div>
            <p className="eyebrow">Studio</p>
            <h2 id="settings-title">Settings</h2>
          </div>
          <button className="icon-button" onClick={onClose} aria-label="Close settings" autoFocus>
            <Icon name="x" />
          </button>
        </header>
        <div className="drawer-content">
          <section>
            <h3>File locations</h3>
            <label>
              <span>Music folder</span>
              <input
                value={setup.input_dir}
                onChange={(event) => setSetup({ ...setup, input_dir: event.target.value })}
              />
            </label>
            <label>
              <span>Corrected copies</span>
              <input
                value={setup.output_dir}
                onChange={(event) => setSetup({ ...setup, output_dir: event.target.value })}
              />
            </label>
          </section>
          <section>
            <h3>Automatic cleaning</h3>
            <label className={`automation-toggle ${setup.automatic_scan_enabled ? "enabled" : ""}`}>
              <input
                type="checkbox"
                checked={setup.automatic_scan_enabled}
                onChange={(event) =>
                  setSetup({ ...setup, automatic_scan_enabled: event.target.checked })
                }
              />
              <span className="toggle-ui" />
              <span>
                <b>
                  {setup.automatic_scan_enabled
                    ? "Scan and write automatically"
                    : "Automatic cleaning is off"}
                </b>
                <small>
                  Confident matches are written automatically. Uncertain music always stays in
                  Review. Automatic work pauses while this web app is open.
                </small>
              </span>
            </label>
            {setup.automatic_scan_enabled && (
              <label className="automation-interval">
                <span>Run every</span>
                <input
                  type="number"
                  min="1"
                  max="1440"
                  value={setup.automatic_scan_interval_minutes}
                  onChange={(event) =>
                    setSetup({
                      ...setup,
                      automatic_scan_interval_minutes: Math.min(
                        1440,
                        Math.max(1, Number(event.target.value) || 1),
                      ),
                    })
                  }
                />
                <span>minutes</span>
              </label>
            )}
          </section>
          <ProviderStatus sources={setup.sources} />
          <details className="source-key-settings">
            <summary>
              Optional source credentials <Icon name="chevron" size={16} />
            </summary>
            <p>
              Free catalogs work without keys. Add credentials only to improve hard-to-identify
              tracks.
            </p>
            <div className="key-grid">
              <Secret
                label="AcoustID"
                active={setup.sources.acoustid}
                value={keys.acoustid}
                onChange={(value) => setKeys({ ...keys, acoustid: value })}
              />
              <Secret
                label="AudD token"
                active={setup.sources.audd}
                value={keys.audd}
                onChange={(value) => setKeys({ ...keys, audd: value })}
              />
              <Secret
                label="Spotify client ID"
                active={setup.sources.spotify}
                value={keys.spotify_client_id}
                onChange={(value) => setKeys({ ...keys, spotify_client_id: value })}
              />
              <Secret
                label="Spotify client secret"
                active={setup.sources.spotify}
                value={keys.spotify_client_secret}
                onChange={(value) => setKeys({ ...keys, spotify_client_secret: value })}
              />
              <Secret
                label="SoundCloud client ID"
                active={setup.sources.soundcloud_search}
                value={keys.soundcloud_client_id}
                onChange={(value) => setKeys({ ...keys, soundcloud_client_id: value })}
              />
              <Secret
                label="SoundCloud secret"
                active={setup.sources.soundcloud_search}
                value={keys.soundcloud_client_secret}
                onChange={(value) => setKeys({ ...keys, soundcloud_client_secret: value })}
              />
              <Secret
                label="YouTube API key"
                active={setup.sources.youtube}
                value={keys.youtube}
                onChange={(value) => setKeys({ ...keys, youtube: value })}
              />
              <Secret
                label="Discogs token"
                active={setup.sources.discogs}
                value={keys.discogs}
                onChange={(value) => setKeys({ ...keys, discogs: value })}
              />
              <Secret
                label="Last.fm key"
                active={setup.sources.lastfm}
                value={keys.lastfm}
                onChange={(value) => setKeys({ ...keys, lastfm: value })}
              />
              <Secret
                label="TheAudioDB key"
                active={setup.sources.theaudiodb}
                value={keys.theaudiodb}
                onChange={(value) => setKeys({ ...keys, theaudiodb: value })}
              />
            </div>
          </details>
        </div>
        <footer>
          <button
            className="primary-action"
            disabled={saving || !setup.input_dir.trim() || !setup.output_dir.trim()}
            onClick={() => void onSave()}
          >
            {saving ? <span className="spinner" /> : <Icon name="check" />}Save settings
          </button>
        </footer>
      </aside>
    </div>
  );
}

function ProviderStatus({ sources }: { sources: Record<string, boolean> }) {
  const available = Object.values(sources).filter(Boolean).length;
  return (
    <section className="provider-settings">
      <div className="section-heading">
        <div>
          <h3>Recognition tools</h3>
          <p>{available} sources and local tools available</p>
        </div>
        <span className="health-dot">Ready</span>
      </div>
      <div className="provider-pills">
        {[
          "musicbrainz",
          "deezer",
          "itunes",
          "radiojavan",
          "navahang",
          "audiomack",
          "genius",
          "spotify",
          "acoustid",
          "shazam",
          "songrec",
          "ffmpeg",
        ].map((source) => (
          <span className={sources[source] ? "active" : "inactive"} key={source}>
            <i />
            {providerName(source)}
          </span>
        ))}
      </div>
      {!sources.songrec && (
        <p className="tool-warning">
          <Icon name="alert" size={15} />
          Install SongRec to recognize difficult files through Shazam. Catalog matching still works
          without it.
        </p>
      )}
      {!sources.ffmpeg && (
        <p className="tool-warning">
          <Icon name="alert" size={15} />
          Install FFmpeg for audio integrity checks and ReplayGain. Metadata cleaning still works.
        </p>
      )}
    </section>
  );
}
