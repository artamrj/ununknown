use crate::domain::credits::ArtistCredit;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPhase {
    #[default]
    Idle,
    Scan,
    Fetch,
    Preview,
    Apply,
    Finish,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum TrackStage {
    #[default]
    Discovered,
    Ready,
    Review,
    Skipped,
    Failed,
}

impl fmt::Display for TrackStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}",
            serde_json::to_value(self).unwrap().as_str().unwrap()
        )
    }
}

impl FromStr for TrackStage {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(&format!("\"{value}\"")).map_err(|error| error.to_string())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, sqlx::Type)]
#[sqlx(transparent)]
pub struct TrackId(pub i64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, sqlx::Type)]
#[sqlx(transparent)]
pub struct CandidateId(pub i64);

macro_rules! numeric_id {
    ($name:ty) => {
        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{}", self.0)
            }
        }
        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_i64(self.0)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Ok(Self(i64::deserialize(deserializer)?))
            }
        }
    };
}

numeric_id!(TrackId);
numeric_id!(CandidateId);

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

/// Apply the canonical fallback tags used for standalone singles.
///
/// Returns which fields were changed so metadata-completion reports can include
/// the generated values.
pub fn normalize_release_fields(candidate: &mut Candidate) -> (bool, bool) {
    let original_album = candidate.album.clone();
    let confirmed_single = candidate
        .release_type
        .as_deref()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("single"));
    if confirmed_single
        || candidate
            .album
            .as_deref()
            .is_none_or(|album| album.trim().is_empty())
    {
        candidate.album = Some("Single".to_owned());
    } else if let Some(album) = candidate.album.as_deref() {
        let cleaned = crate::domain::credits::release_title_without_featured(album);
        candidate.album = Some(if cleaned.is_empty() {
            "Single".to_owned()
        } else {
            cleaned
        });
    }
    let album_defaulted = candidate.album != original_album;

    let album_artist_defaulted = candidate
        .album_artist
        .as_deref()
        .is_none_or(|album_artist| album_artist.trim().is_empty());
    if album_artist_defaulted {
        if candidate.is_compilation {
            candidate.album_artist = Some("Various Artists".into());
            candidate.album_artist_credits = vec![ArtistCredit::new("Various Artists", "")];
        } else {
            candidate.album_artist = Some(candidate.artist.clone());
            candidate.album_artist_credits = candidate.artist_credits.clone();
        }
    } else if candidate.album_artist_credits.is_empty()
        && let Some(album_artist) = candidate.album_artist.as_deref()
    {
        let credits = crate::domain::credits::normalize_featured(album_artist, "");
        candidate.album_artist = Some(credits.artist);
        candidate.album_artist_credits = credits.artists;
    }

    (album_defaulted, album_artist_defaulted)
}
