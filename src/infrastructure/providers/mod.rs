pub mod acoustid;
pub mod audd;
pub mod audiomack;
pub mod cover_art_archive;
pub mod deezer;
pub mod discogs;
pub mod genius;
pub mod itunes;
pub mod lastfm;
pub mod musicbrainz;
pub mod navahang;
pub mod radiojavan;
pub mod shazam;
pub mod songrec;
pub mod soundcloud;
pub mod spotify;
pub mod theaudiodb;
pub mod wikidata;
pub mod youtube;

use serde::{Deserialize, Serialize};

pub use crate::domain::credits::ArtistCredit;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtworkStatus {
    #[default]
    Searching,
    Verified,
    RetryableError,
    CoverRequired,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtworkCandidate {
    pub provider: String,
    pub url: String,
    pub user_confirmed: bool,
    pub release_id: Option<String>,
    pub isrc: Option<String>,
    pub album: Option<String>,
    pub artist: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Candidate {
    pub id: Option<i64>,
    pub provider: String,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub artist_credits: Vec<ArtistCredit>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    #[serde(default)]
    pub album_artist_credits: Vec<ArtistCredit>,
    pub track_number: Option<i64>,
    pub track_total: Option<i64>,
    pub disc_number: Option<i64>,
    pub disc_total: Option<i64>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub composer: Option<String>,
    pub label: Option<String>,
    pub isrc: Option<String>,
    pub cover_url: Option<String>,
    #[serde(default)]
    pub artwork_candidates: Vec<ArtworkCandidate>,
    #[serde(default)]
    pub artwork_status: ArtworkStatus,
    pub artwork_message: Option<String>,
    pub recording_id: Option<String>,
    pub release_id: Option<String>,
    pub release_country: Option<String>,
    pub release_date: Option<String>,
    pub release_type: Option<String>,
    pub release_secondary_types: Option<String>,
    pub is_compilation: bool,
    pub duration_delta: Option<f64>,
    pub score_breakdown: Option<String>,
    pub artist_id: Option<String>,
    pub album_artist_id: Option<String>,
    pub score: f64,
    #[serde(skip_serializing)]
    pub raw_json: String,
}
