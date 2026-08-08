import { memo } from "react";
import { Icon } from "../Icons";
import type { Track } from "@/api/types";
import { fileStem, isCompleted, outputFilename, selectedCandidate } from "../trackUtils";
import { Artwork, OriginalArtwork, TrackStatus } from "./TrackInspector";

function TrackRowBase({
  track,
  active,
  onSelect,
}: {
  track: Track;
  active: boolean;
  onSelect: (track: Track) => void;
}) {
  const chosen = selectedCandidate(track);
  const displayTitle = chosen?.title || track.current_title || fileStem(track.filename);
  const artist = chosen?.artist || track.current_artist || "Unknown artist";
  const album = chosen?.album || track.current_album || "Album not set";
  const filename = chosen ? outputFilename(track, chosen) : track.filename;
  return (
    <button
      className={`track-row ${active ? "selected" : ""}`}
      onClick={() => onSelect(track)}
      role="listitem"
      aria-current={active ? "true" : undefined}
    >
      {chosen ? (
        <Artwork candidate={chosen} trackId={track.id} size="small" />
      ) : (
        <OriginalArtwork track={track} size="small" />
      )}
      <span className="row-identity">
        <b>{displayTitle}</b>
        <small>
          {artist}
          <i>·</i>
          {album}
        </small>
      </span>
      <span className={`row-file ${chosen ? "chosen" : ""}`}>
        <small>
          {chosen ? (isCompleted(track) ? "Written file" : "New filename") : "Original file"}
        </small>
        <b title={filename}>{filename}</b>
      </span>
      <TrackStatus track={track} />
      <Icon name="chevron" size={16} className="row-chevron" />
    </button>
  );
}

export const TrackRow = memo(TrackRowBase);
