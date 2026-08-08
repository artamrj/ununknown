import { useState } from "react";
import { api } from "@/api/client";
import type { ArtistCredit, Candidate, Track } from "@/api/types";
import { Icon } from "../Icons";

export function ManualEditor({
  track,
  candidate,
  onSaved,
}: {
  track: Track;
  candidate?: Candidate;
  onSaved: () => Promise<void>;
}) {
  const [form, setForm] = useState<Record<string, string | number>>({
    title: candidate?.title || track.current_title || "",
    artist: candidate?.artist || track.current_artist || "",
    album: candidate?.album || track.current_album || "",
    album_artist: candidate?.album_artist || track.current_album_artist || "",
    track_number: candidate?.track_number || track.current_track_number || "",
    year: candidate?.year || "",
    release_date: candidate?.release_date || "",
    genre: candidate?.genre || "",
    composer: candidate?.composer || "",
    label: candidate?.label || "",
    isrc: candidate?.isrc || "",
    cover_url: candidate?.cover_url || "",
  });
  const [sourceUrl, setSourceUrl] = useState("");
  const [artistCredits, setArtistCredits] = useState<ArtistCredit[]>(
    candidate?.artist_credits?.length
      ? candidate.artist_credits
      : [{ name: candidate?.artist || track.current_artist || "", join_phrase: "" }],
  );
  const [albumArtistCredits, setAlbumArtistCredits] = useState<ArtistCredit[]>(
    candidate?.album_artist_credits?.length
      ? candidate.album_artist_credits
      : [{ name: candidate?.album_artist || track.current_album_artist || "", join_phrase: "" }],
  );
  const [feedback, setFeedback] = useState<{ error?: boolean; text: string }>();
  const [resolving, setResolving] = useState(false);
  const [saving, setSaving] = useState(false);
  const resolveSource = async () => {
    setResolving(true);
    setFeedback(undefined);
    try {
      const found = await api<Candidate>("/source/resolve", {
        method: "POST",
        body: JSON.stringify({ url: sourceUrl }),
      });
      setForm((current) => ({
        ...current,
        title: found.title || current.title,
        artist: found.artist || current.artist,
        album: found.album || current.album,
        album_artist: found.album_artist || current.album_artist,
        track_number: found.track_number || current.track_number,
        year: found.year || current.year,
        release_date: found.release_date || current.release_date,
        genre: found.genre || current.genre,
        composer: found.composer || current.composer,
        label: found.label || current.label,
        isrc: found.isrc || current.isrc,
        cover_url: found.cover_url || current.cover_url,
      }));
      if (found.artist_credits?.length) setArtistCredits(found.artist_credits);
      if (found.album_artist_credits?.length)
        setAlbumArtistCredits(found.album_artist_credits);
      setFeedback({ text: `Loaded metadata and artwork for ${found.artist || "this track"}.` });
    } catch (reason) {
      setFeedback({ error: true, text: (reason as Error).message });
    } finally {
      setResolving(false);
    }
  };
  const save = async () => {
    setSaving(true);
    setFeedback(undefined);
    try {
      await api(`/tracks/${track.id}/manual`, {
        method: "PUT",
        body: JSON.stringify({
          ...form,
          artist_credits: artistCredits.filter((credit) => credit.name.trim()),
          album_artist_credits: albumArtistCredits.filter((credit) => credit.name.trim()),
          track_number: form.track_number ? Number(form.track_number) : null,
        }),
      });
      await onSaved();
    } catch (reason) {
      setFeedback({ error: true, text: (reason as Error).message });
    } finally {
      setSaving(false);
    }
  };
  const labels: Record<string, string> = {
    title: "Title",
    artist: "Artist",
    album: "Album",
    album_artist: "Album artist",
    track_number: "Track number",
    year: "Release year",
    release_date: "Release date",
    genre: "Genre",
    composer: "Composer",
    label: "Label",
    isrc: "ISRC",
    cover_url: "Cover artwork URL",
  };
  return (
    <section className="manual-editor" aria-labelledby="manual-title">
      <div className="section-heading">
        <div>
          <p className="eyebrow">Manual correction</p>
          <h3 id="manual-title">Edit proposed metadata</h3>
        </div>
      </div>
      <div className="source-resolver">
        <label>
          <span>Use a track link</span>
          <input
            value={sourceUrl}
            onChange={(event) => setSourceUrl(event.target.value)}
            placeholder="Shazam, Spotify, SoundCloud, Audiomack, Navahang, Genius, Radio Javan, or YouTube URL"
          />
        </label>
        <button
          className="compact-button"
          disabled={!sourceUrl.trim() || resolving}
          onClick={() => void resolveSource()}
        >
          {resolving ? "Loading…" : "Import"}
        </button>
      </div>
      {feedback && (
        <p
          className={`inline-feedback ${feedback.error ? "error" : "success"}`}
          role={feedback.error ? "alert" : "status"}
        >
          {feedback.text}
        </p>
      )}
      <div className="editor-grid">
        {Object.entries(form).map(([name, value]) => (
          <label className={name === "cover_url" ? "wide" : ""} key={name}>
            <span>{labels[name]}</span>
            <input
              value={value}
              onChange={(event) => setForm({ ...form, [name]: event.target.value })}
            />
          </label>
        ))}
      </div>
      <div className="editor-grid">
        <ArtistCreditsEditor
          label="Individual track artists"
          credits={artistCredits}
          onChange={setArtistCredits}
        />
        <ArtistCreditsEditor
          label="Individual album artists"
          credits={albumArtistCredits}
          onChange={setAlbumArtistCredits}
        />
      </div>
      <button
        className="primary-action editor-save"
        disabled={saving || !String(form.title).trim() || !String(form.artist).trim()}
        onClick={() => void save()}
      >
        {saving ? <span className="spinner" /> : <Icon name="check" />}Use this metadata
      </button>
    </section>
  );
}

function ArtistCreditsEditor({
  label,
  credits,
  onChange,
}: {
  label: string;
  credits: ArtistCredit[];
  onChange: (credits: ArtistCredit[]) => void;
}) {
  const update = (index: number, name: string) =>
    onChange(credits.map((credit, position) => (position === index ? { ...credit, name } : credit)));
  const remove = (index: number) => {
    const next = credits.filter((_, position) => position !== index);
    if (next.length) next[next.length - 1] = { ...next[next.length - 1], join_phrase: "" };
    onChange(next);
  };
  const add = () => {
    const next = credits.map((credit, index) =>
      index + 1 === credits.length ? { ...credit, join_phrase: " / " } : credit,
    );
    onChange([...next, { name: "", join_phrase: "" }]);
  };
  return (
    <fieldset className="credit-editor wide">
      <legend>{label}</legend>
      <div className="credit-chips">
        {credits.map((credit, index) => (
          <span className="credit-chip" key={`${index}-${credit.musicbrainz_id || "manual"}`}>
            <input
              aria-label={`${label} ${index + 1}`}
              value={credit.name}
              onChange={(event) => update(index, event.target.value)}
            />
            <button type="button" aria-label={`Remove ${credit.name}`} onClick={() => remove(index)}>
              ×
            </button>
          </span>
        ))}
        <button type="button" className="compact-button" onClick={add}>
          Add artist
        </button>
      </div>
    </fieldset>
  );
}

