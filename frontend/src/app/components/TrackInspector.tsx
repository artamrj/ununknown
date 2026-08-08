import { useEffect, useState } from "react";
import { api } from "@/api/client";
import type { Candidate, Track } from "@/api/types";
import { Icon, type IconName } from "../Icons";
import {
  artworkStatusLabel,
  artworkUrls,
  candidateConfidence,
  candidateSignals,
  candidateSources,
  candidateVerdict,
  fileStem,
  formatDuration,
  friendlyTrackError,
  googleMetadataUrl,
  hasRetryableArtwork,
  isCompleted,
  isProblem,
  isReview,
  metadataAudit,
  outputFilename,
  problemTitle,
  selectedCandidate,
  statusFor,
} from "../trackUtils";
import { ManualEditor } from "./ManualEditor";

export function TrackInspector({
  track,
  onChoose,
  onSaved,
}: {
  track: Track;
  onChoose: (trackId: number, candidateId: number) => Promise<void>;
  onSaved: () => Promise<void>;
}) {
  const [editing, setEditing] = useState(false);
  const [advanced, setAdvanced] = useState(false);
  const [undoing, setUndoing] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [searchingCover, setSearchingCover] = useState(false);
  const [savingCover, setSavingCover] = useState(false);
  const [coverUrl, setCoverUrl] = useState("");
  const [choosingId, setChoosingId] = useState<number>();
  const [actionError, setActionError] = useState("");
  const candidate = selectedCandidate(track);
  const corrupt = track.status === "corrupt";
  const showChosenOverview = Boolean(candidate);
  const overviewTitle = showChosenOverview ? candidate?.title : track.current_title;
  const overviewArtist = showChosenOverview ? candidate?.artist : track.current_artist;
  const overviewAlbum = showChosenOverview ? candidate?.album : track.current_album;
  const overviewAlbumArtist = showChosenOverview
    ? candidate?.album_artist
    : track.current_album_artist;
  const overviewTrackNumber = showChosenOverview
    ? candidate?.track_number
    : track.current_track_number;
  const overviewFilename =
    showChosenOverview && candidate ? outputFilename(track, candidate) : track.filename;
  useEffect(() => {
    setEditing(false);
    setAdvanced(false);
    setActionError("");
    setChoosingId(undefined);
    setRemoving(false);
    setSearchingCover(false);
    setSavingCover(false);
    setCoverUrl(candidate?.cover_url || "");
  }, [track.id, candidate?.cover_url]);
  const undoIdentification = async () => {
    setUndoing(true);
    setActionError("");
    try {
      await api(`/tracks/${track.id}/review`, { method: "POST", body: "{}" });
      await onSaved();
    } catch (reason) {
      setActionError((reason as Error).message);
    } finally {
      setUndoing(false);
    }
  };
  const acceptCandidate = async (candidateId: number) => {
    setChoosingId(candidateId);
    setActionError("");
    try {
      await onChoose(track.id, candidateId);
    } catch (reason) {
      setActionError((reason as Error).message);
    } finally {
      setChoosingId(undefined);
    }
  };
  const removeMusicFile = async () => {
    if (
      !confirm(
        `Permanently remove "${track.filename}" from the music folder? This cannot be undone.`,
      )
    )
      return;
    setRemoving(true);
    setActionError("");
    try {
      await api(`/tracks/${track.id}`, { method: "DELETE" });
      await onSaved();
    } catch (reason) {
      setActionError((reason as Error).message);
    } finally {
      setRemoving(false);
    }
  };
  const searchCover = async () => {
    setSearchingCover(true);
    setActionError("");
    try {
      await api(`/tracks/${track.id}/artwork/search`, { method: "POST", body: "{}" });
      await onSaved();
    } catch (reason) {
      setActionError((reason as Error).message);
    } finally {
      setSearchingCover(false);
    }
  };
  const saveManualCover = async () => {
    const url = coverUrl.trim();
    if (!url) return;
    setSavingCover(true);
    setActionError("");
    try {
      await api(`/tracks/${track.id}/artwork`, {
        method: "PUT",
        body: JSON.stringify({ cover_url: url }),
      });
      await onSaved();
    } catch (reason) {
      setActionError((reason as Error).message);
    } finally {
      setSavingCover(false);
    }
  };
  const coverBusy = searchingCover || savingCover;
  const coverNeeded = Boolean(
    candidate && !isCompleted(track) && candidate.artwork_status !== "verified",
  );

  return (
    <div className="inspector-content">
      <header className="original-overview">
        {showChosenOverview && candidate ? (
          <Artwork candidate={candidate} trackId={track.id} size="large" />
        ) : (
          <OriginalArtwork track={track} size="large" />
        )}
        <div className="original-overview-main">
          <div className="overview-label">
            <span>
              {showChosenOverview
                ? isCompleted(track)
                  ? "Written metadata"
                  : "Chosen metadata"
                : "Original file"}
            </span>
            <TrackStatus track={track} />
          </div>
          <h2>{overviewTitle || fileStem(track.filename)}</h2>
          <p>
            {overviewArtist || "Unknown artist"}
            <i>·</i>
            {overviewAlbum || "Album not set"}
          </p>
          <div className={`original-filename ${showChosenOverview ? "chosen" : ""}`}>
            <Icon name="music" size={15} />
            <span>
              <small>
                {showChosenOverview
                  ? isCompleted(track)
                    ? "Written filename"
                    : "New filename"
                  : "Filename"}
              </small>
              <b title={overviewFilename}>{overviewFilename}</b>
            </span>
          </div>
          <div className="original-audio-facts">
            <span>
              <small>Album artist</small>
              <b>{overviewAlbumArtist || "Not set"}</b>
            </span>
            <span>
              <small>Track</small>
              <b>
                {overviewTrackNumber
                  ? `${overviewTrackNumber}${showChosenOverview && candidate?.track_total ? ` / ${candidate.track_total}` : ""}`
                  : "Not set"}
              </b>
            </span>
            <span>
              <small>Audio</small>
              <b>
                {[track.format?.toUpperCase(), formatDuration(track.duration)]
                  .filter(Boolean)
                  .join(" · ") || "Unknown"}
              </b>
            </span>
          </div>
        </div>
      </header>

      {!track.is_missing && !corrupt && track.stage !== "skipped" && (
        <audio
          className="audio-player"
          controls
          preload="none"
          src={`/api/tracks/${track.id}/audio`}
          aria-label={`Play ${track.filename}`}
        />
      )}

      {(isReview(track) || isProblem(track) || track.stage === "skipped") && (
        <section
          className={`decision-note ${isProblem(track) ? "problem" : "review"} ${coverNeeded ? "cover-needed" : ""}`}
        >
          <Icon
            name={
              isProblem(track)
                ? "alert"
                : track.stage === "skipped"
                  ? "skip"
                  : coverNeeded
                    ? "album"
                    : "info"
            }
          />
          <div>
            <b>
              {isProblem(track)
                ? problemTitle(track)
                : track.stage === "skipped"
                  ? "Duplicate input"
                  : hasRetryableArtwork(track)
                    ? "Cover download temporarily unavailable"
                    : coverNeeded
                      ? "A verified cover is required"
                      : "Your review is needed"}
            </b>
            <p>
              {track.stage_message ||
                friendlyTrackError(track) ||
                "The available matches are too close to choose safely."}
            </p>
            {coverNeeded && (
              <div className="cover-search-actions">
                <div className="cover-search-buttons">
                  <button
                    className="compact-button accent"
                    disabled={coverBusy}
                    onClick={() => void searchCover()}
                  >
                    {searchingCover ? (
                      <span className="spinner" />
                    ) : (
                      <Icon name="refresh" size={15} />
                    )}
                    {searchingCover ? "Searching…" : "Search for cover art"}
                  </button>
                </div>
                <label className="cover-url-field">
                  <span>Or paste a cover URL</span>
                  <span className="cover-url-row">
                    <input
                      value={coverUrl}
                      onChange={(event) => setCoverUrl(event.target.value)}
                      placeholder="HTTPS image, Shazam, Spotify, Genius, Radio Javan…"
                      disabled={coverBusy}
                      onKeyDown={(event) => {
                        if (event.key === "Enter") {
                          event.preventDefault();
                          void saveManualCover();
                        }
                      }}
                    />
                    <button
                      className="compact-button accent"
                      disabled={coverBusy || !coverUrl.trim()}
                      onClick={() => void saveManualCover()}
                    >
                      {savingCover ? <span className="spinner" /> : <Icon name="check" size={15} />}
                      {savingCover ? "Saving…" : "Use this cover"}
                    </button>
                  </span>
                </label>
                <small>
                  Auto-search checks catalogs and covers already found for the same release. You can
                  also paste an image or song-link cover URL.
                </small>
              </div>
            )}
            {track.error && track.stage_message && (
              <details className="technical-error">
                <summary>Technical details</summary>
                <code>{track.error}</code>
              </details>
            )}
          </div>
        </section>
      )}

      {isReview(track) && (
        <div className="review-file-actions">
          <span>
            <b>Don’t want this file in your library?</b>
            <small>Remove the original music file and this review item.</small>
          </span>
          <button
            className="remove-file-button"
            disabled={removing}
            onClick={() => void removeMusicFile()}
          >
            <Icon name="trash" size={15} />
            {removing ? "Removing…" : "Remove music file"}
          </button>
        </div>
      )}

      {isReview(track) && track.candidates.length > 0 && (
        <section className="inspector-section candidate-section">
          <div className="section-heading">
            <div>
              <p className="eyebrow">Ununknown suggestions</p>
              <h3>Choose the recording that matches this file</h3>
            </div>
            <span>{track.candidates.length} options</span>
          </div>
          <p className="candidate-help">
            Each option shows the exact differences from the original tags. Green evidence agrees;
            amber or red evidence needs your attention.
          </p>
          <div className="candidate-list">
            {track.candidates.slice(0, 5).map((item, index) => (
              <CandidateRow
                key={item.id}
                track={track}
                candidate={item}
                rank={index + 1}
                topScore={track.candidates[0]?.score || item.score}
                nextScore={index === 0 ? track.candidates[1]?.score : undefined}
                choosing={choosingId === item.id}
                disabled={choosingId !== undefined}
                onChoose={() => void acceptCandidate(item.id)}
              />
            ))}
          </div>
        </section>
      )}

      {actionError && (
        <p className="inline-feedback error" role="alert">
          {actionError}
        </p>
      )}

      {candidate && (
        <>
          <section className="inspector-section selected-proposal">
            <div className="section-heading">
              <div>
                <p className="eyebrow">Ununknown suggestion</p>
                <h3>Review exactly what will change</h3>
              </div>
              <button className="text-button" onClick={() => setEditing(!editing)}>
                <Icon name="edit" size={14} />
                {editing ? "Close editor" : "Edit suggestion"}
              </button>
            </div>
            <div className="proposal-summary">
              <div className="proposal-side current">
                <OriginalArtwork track={track} size="medium" />
                <span>
                  <small>Current file</small>
                  <b>{track.current_title || fileStem(track.filename)}</b>
                  <em>{track.current_artist || "Unknown artist"}</em>
                </span>
              </div>
              <Icon name="arrow" size={17} />
              <div className="proposal-side proposed">
                <Artwork candidate={candidate} trackId={track.id} size="medium" />
                <span>
                  <small>Will be written</small>
                  <b>{candidate.title || "Untitled"}</b>
                  <em>{candidate.artist || "Unknown artist"}</em>
                </span>
                <ConfidenceBadge score={candidate.score} />
              </div>
            </div>
            <div className="metadata-comparison">
              <CompareRow label="Title" before={track.current_title} after={candidate.title} />
              <CompareRow label="Artist" before={track.current_artist} after={candidate.artist} />
              <CompareRow label="Album" before={track.current_album} after={candidate.album} />
              <CompareRow
                label="Album artist"
                before={track.current_album_artist}
                after={candidate.album_artist}
              />
              <CompareRow
                label="Track"
                before={track.current_track_number?.toString()}
                after={
                  candidate.track_number
                    ? `${candidate.track_number}${candidate.track_total ? ` / ${candidate.track_total}` : ""}`
                    : undefined
                }
              />
              <CompareRow
                label="Year"
                after={candidate.year || candidate.release_date?.slice(0, 4)}
              />
              <CompareRow label="Genre" after={candidate.genre} />
            </div>
            <div className="filename-comparison">
              <span>
                <small>Current filename</small>
                <b title={track.filename}>{track.filename}</b>
              </span>
              <Icon name="arrow" size={15} />
              <span>
                <small>New filename</small>
                <b>{outputFilename(track, candidate)}</b>
              </span>
            </div>
          </section>
          <section className="quality-strip">
            <QualityItem icon="layers" label="Sources" value={candidateSources(candidate)} />
            <QualityItem icon="album" label="Artwork" value={artworkStatusLabel(candidate)} />
            <QualityItem icon="waveform" label="ReplayGain" value="Added on write" />
          </section>
        </>
      )}

      {editing && (
        <ManualEditor
          track={track}
          candidate={candidate}
          onSaved={async () => {
            await onSaved();
            setEditing(false);
          }}
        />
      )}

      {!corrupt && !editing && (
        <button className="manual-entry" onClick={() => setEditing(true)}>
          <Icon name="edit" />
          <span>
            <b>{candidate ? "Fine-tune metadata" : "Enter metadata manually"}</b>
            <small>Use a source link or fill in fields yourself</small>
          </span>
          <Icon name="chevron" />
        </button>
      )}

      {candidate && (
        <details
          className="advanced-details"
          open={advanced}
          onToggle={(event) => setAdvanced(event.currentTarget.open)}
        >
          <summary>
            Source evidence and advanced details <Icon name="chevron" size={15} />
          </summary>
          <div>
            <MetadataHealth candidate={candidate} />
            <GoogleCheck candidate={candidate} />
            <dl>
              <div>
                <dt>Provider</dt>
                <dd>{candidateSources(candidate)}</dd>
              </div>
              <div>
                <dt>ISRC</dt>
                <dd>{candidate.isrc || "Not available"}</dd>
              </div>
              <div>
                <dt>Label</dt>
                <dd>{candidate.label || "Not available"}</dd>
              </div>
              <div>
                <dt>Release date</dt>
                <dd>{candidate.release_date || candidate.year || "Not available"}</dd>
              </div>
            </dl>
          </div>
        </details>
      )}

      {candidate && (
        <div className="inspector-actions">
          <span>
            <Icon name="check" />
            {isCompleted(track) ? "Corrected file written" : "Ready to write"}
          </span>
          <button
            className="text-button warning"
            disabled={undoing}
            onClick={() => void undoIdentification()}
          >
            {undoing ? "Returning…" : "Return to review"}
          </button>
        </div>
      )}
    </div>
  );
}

function CandidateRow({
  track,
  candidate,
  rank,
  topScore,
  nextScore,
  choosing,
  disabled,
  onChoose,
}: {
  track: Track;
  candidate: Candidate;
  rank: number;
  topScore: number;
  nextScore?: number;
  choosing: boolean;
  disabled: boolean;
  onChoose: () => void;
}) {
  const audit = metadataAudit(candidate);
  const confidence = candidateConfidence(candidate.score);
  const signals = candidateSignals(track, candidate);
  const gap =
    rank === 1 && typeof nextScore === "number"
      ? Math.max(0, candidate.score - nextScore)
      : Math.max(0, topScore - candidate.score);
  const recommended =
    rank === 1 && candidate.score >= 85 && (typeof nextScore !== "number" || gap >= 8);
  const verdict = candidateVerdict(candidate.score, signals);
  return (
    <article className={`candidate-row ${recommended ? "recommended" : ""}`}>
      <header className="candidate-main">
        <span className="candidate-rank" aria-label={`Option ${rank}`}>
          {rank}
        </span>
        <Artwork candidate={candidate} size="medium" />
        <div className="candidate-identity">
          <span className="candidate-kicker">{recommended ? "Recommended" : `Option ${rank}`}</span>
          <b>{candidate.title || "Untitled"}</b>
          <span>{candidate.artist || "Unknown artist"}</span>
          <small>
            {[
              candidate.album || "Album unknown",
              candidate.year || candidate.release_date?.slice(0, 4),
              candidate.track_number
                ? `Track ${candidate.track_number}${candidate.track_total ? ` of ${candidate.track_total}` : ""}`
                : "",
            ]
              .filter(Boolean)
              .join(" · ")}
          </small>
        </div>
        <div className={`candidate-score ${confidence.tone}`}>
          <span>{confidence.label}</span>
          <strong>{Math.round(candidate.score)}%</strong>
          <small>
            {rank === 1
              ? typeof nextScore === "number"
                ? `${Math.round(gap)} points above #2`
                : "Only result"
              : `${Math.round(gap)} points below #1`}
          </small>
        </div>
        <button className="accept-button" disabled={disabled} onClick={onChoose}>
          {choosing ? <span className="spinner" /> : <Icon name="check" size={15} />}
          {choosing ? "Choosing…" : "Choose"}
        </button>
      </header>
      <div className={`candidate-verdict ${verdict.tone}`}>
        <Icon name={verdict.tone === "good" ? "check" : "alert"} size={14} />
        <b>{verdict.title}</b>
        <span>{verdict.detail}</span>
      </div>
      <div
        className="candidate-comparison"
        aria-label={`Compare option ${rank} with original file`}
      >
        <div className="comparison-head">
          <span>Field</span>
          <span>In your file</span>
          <span>This suggestion</span>
          <span>Evidence</span>
        </div>
        <CandidateCompareRow
          label="Title"
          original={track.current_title}
          proposed={candidate.title}
          signal={signals[0]}
        />
        <CandidateCompareRow
          label="Artist"
          original={track.current_artist}
          proposed={candidate.artist}
          signal={signals[1]}
        />
        <CandidateCompareRow
          label="Album"
          original={track.current_album}
          proposed={candidate.album}
          signal={signals[2]}
        />
        <CandidateCompareRow
          label="Audio length"
          original={formatDuration(track.duration)}
          proposed={signals[3].value}
          signal={signals[3]}
        />
      </div>
      <footer className="candidate-meta">
        <span>{candidateSources(candidate)}</span>
        <i>·</i>
        <span>{audit.coreComplete ? "Complete metadata" : `${audit.score}% metadata`}</span>
        <i>·</i>
        <span title={candidate.artwork_message}>{artworkStatusLabel(candidate)}</span>
        {candidate.isrc && (
          <>
            <i>·</i>
            <span>ISRC {candidate.isrc}</span>
          </>
        )}
      </footer>
    </article>
  );
}

function CandidateCompareRow({
  label,
  original,
  proposed,
  signal,
}: {
  label: string;
  original?: string;
  proposed?: string;
  signal: { value: string; tone: string };
}) {
  return (
    <div className="candidate-compare-row">
      <small>{label}</small>
      <b className={!original ? "empty" : ""} title={original}>
        {original || "Not set"}
      </b>
      <b className={!proposed ? "empty" : ""} title={proposed}>
        {proposed || "Not found"}
      </b>
      <span className={signal.tone}>{signal.value}</span>
    </div>
  );
}

function ConfidenceBadge({ score }: { score: number }) {
  const confidence = candidateConfidence(score);
  return (
    <span className={`confidence-badge ${confidence.tone}`}>
      <b>{Math.round(score)}%</b>
      {confidence.label}
    </span>
  );
}

function CompareRow({ label, before, after }: { label: string; before?: string; after?: string }) {
  const changed = Boolean(after && after !== before);
  return (
    <div className="compare-row">
      <span>{label}</span>
      <p className={!before ? "empty" : ""} title={before}>
        {before || "Not set"}
      </p>
      <Icon name="arrow" size={14} />
      <p className={`${!after ? "empty" : ""} ${changed ? "changed" : ""}`} title={after}>
        {after || before || "Not found"}
      </p>
      <em className={changed ? "changed" : "same"}>{changed ? "Change" : "Keep"}</em>
    </div>
  );
}

function QualityItem({ icon, label, value }: { icon: IconName; label: string; value: string }) {
  return (
    <div>
      <Icon name={icon} />
      <span>
        <small>{label}</small>
        <b>{value}</b>
      </span>
    </div>
  );
}

export function TrackStatus({ track }: { track: Track }) {
  const status = statusFor(track);
  return (
    <span className={`status-pill ${status.tone}`}>
      <Icon name={status.icon} size={13} />
      {status.label}
    </span>
  );
}

export function InspectorEmpty({ busy }: { busy: boolean }) {
  return (
    <div className="inspector-empty">
      <span>
        <Icon name={busy ? "waveform" : "disc"} size={32} />
      </span>
      <h2>{busy ? "Building your queue" : "Select a track"}</h2>
      <p>
        {busy
          ? "Identification results will appear while the scan continues."
          : "Inspect the original tags, proposed corrections, artwork, sources, and confidence here."}
      </p>
    </div>
  );
}

export function Secret({
  label,
  active,
  value,
  onChange,
}: {
  label: string;
  active?: boolean;
  value?: string;
  onChange: (value: string) => void;
}) {
  return (
    <label>
      <span>
        {label}
        {active && <i>Configured</i>}
      </span>
      <input
        type="password"
        value={value || ""}
        onChange={(event) => onChange(event.target.value)}
        placeholder={active ? "Leave blank to keep" : "Optional"}
        autoComplete="off"
      />
    </label>
  );
}

export function Artwork({
  candidate,
  trackId,
  size = "small",
}: {
  candidate?: Candidate;
  trackId?: number;
  size?: "small" | "medium" | "large";
}) {
  const catalogUrls = artworkUrls(candidate);
  const urls = trackId
    ? [
        `/api/tracks/${trackId}/artwork/preview?v=${encodeURIComponent(candidate?.cover_url || candidate?.id || "embedded")}`,
        ...catalogUrls,
      ]
    : candidate?.id && catalogUrls.length
      ? [
          `/api/candidates/${candidate.id}/artwork/preview?v=${encodeURIComponent(candidate.cover_url || candidate.id)}`,
          ...catalogUrls,
        ]
      : catalogUrls;
  const [index, setIndex] = useState(0);
  useEffect(() => setIndex(0), [candidate?.id, candidate?.cover_url]);
  return urls[index] ? (
    <img
      className={`artwork artwork-${size}`}
      src={urls[index]}
      alt={`Cover for ${candidate?.album || candidate?.title || "track"}`}
      loading="lazy"
      onError={() => setIndex((current) => current + 1)}
    />
  ) : (
    <span className={`artwork artwork-${size} artwork-missing`}>
      <Icon name="music" size={size === "large" ? 28 : 18} />
    </span>
  );
}

export function OriginalArtwork({
  track,
  size = "small",
}: {
  track: Track;
  size?: "small" | "medium" | "large";
}) {
  const [missing, setMissing] = useState(false);
  useEffect(() => setMissing(false), [track.id]);
  return missing ? (
    <span className={`artwork artwork-${size} artwork-missing`}>
      <Icon name="music" size={size === "large" ? 28 : 18} />
    </span>
  ) : (
    <img
      className={`artwork artwork-${size}`}
      src={`/api/tracks/${track.id}/artwork/original?v=${encodeURIComponent(track.filename)}`}
      alt={`Embedded cover from ${track.filename}`}
      loading="lazy"
      onError={() => setMissing(true)}
    />
  );
}

function GoogleCheck({ candidate }: { candidate: Candidate }) {
  return (
    <a
      className="google-check"
      href={googleMetadataUrl(candidate)}
      target="_blank"
      rel="noreferrer"
    >
      <span>Verify release details on Google</span>
      <Icon name="arrow" size={15} />
    </a>
  );
}

function MetadataHealth({ candidate }: { candidate: Candidate }) {
  const audit = metadataAudit(candidate);
  return (
    <span className={`metadata-health ${audit.coreComplete ? "complete" : "incomplete"}`}>
      <Icon name={audit.coreComplete ? "check" : "alert"} size={14} />
      {audit.score}% metadata{" "}
      {audit.coreComplete ? "complete" : `· missing ${audit.missing.slice(0, 3).join(", ")}`}
    </span>
  );
}
