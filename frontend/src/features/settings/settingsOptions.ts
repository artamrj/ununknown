export const tabs = ["Basic", "Matching", "Metadata Sources", "Metadata", "Files & Paths", "Expert"];

export const modeHelp: Record<string, [string, string]> = {
  safe: ["Safe", "Automatically keeps strict matches with a clear release winner."],
  aggressive: ["Aggressive", "Automatically keeps matches at 75% or higher."],
  manual: [
    "Manual",
    "Does not auto-select anything; candidates are sent to review.",
  ],
  custom: ["Custom", "Automatically keeps matches at your chosen confidence number."],
};

export const compilationHelp: Record<string, [string, string]> = {
  avoid: ["Avoid by default", "Penalizes compilations when album or single releases are plausible."],
  allow: ["Allow normally", "Scores compilations without an extra penalty or bonus."],
  prefer: ["Prefer compilations", "Boosts compilation releases and labels them in review."],
};

export const metadataGroups = [
  [
    "Core",
    [
      ["title", "Title", "Song name"],
      ["artist", "Artist", "Track artist"],
      ["album", "Album", "Release title"],
      ["album_artist", "Album artist", "Main release artist"],
    ],
  ],
  [
    "Track Numbers",
    [
      ["track_number", "Track number", "Track position"],
      ["track_total", "Track total", "Total tracks"],
      ["disc_number", "Disc number", "Disc position"],
      ["disc_total", "Disc total", "Total discs"],
    ],
  ],
  [
    "Release Info",
    [
      ["release_date", "Year / date", "Release year or date"],
      ["genre", "Genre", "Genre when available"],
      ["label", "Label", "Record label"],
      ["composer", "Composer", "Composer credit"],
    ],
  ],
  [
    "IDs",
    [
      ["isrc", "ISRC", "Recording code"],
      ["musicbrainz_recording_id", "MusicBrainz recording", "Canonical recording ID"],
      ["musicbrainz_release_id", "MusicBrainz release", "Release ID"],
      ["musicbrainz_artist_id", "MusicBrainz artist", "Artist ID"],
      ["musicbrainz_album_artist_id", "MusicBrainz album artist", "Album artist ID"],
    ],
  ],
  [
    "Extras",
    [
      ["comment", "Comment", "Adds a simple Ununknown comment"],
      ["embed_cover_art", "Embed cover art", "Writes artwork into supported files"],
      ["replace_existing_cover_art", "Replace cover art", "Expert-only cover replacement"],
    ],
  ],
] as const;
