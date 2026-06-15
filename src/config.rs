use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{env, fs, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(skip)]
    pub db_path: String,
    pub input_dir: String,
    pub output_dir: String,
    pub output_mode: String,
    pub automation_mode: String,
    pub confidence_threshold: f64,
    pub cover_art_enabled: bool,
    pub overwrite_existing_tags: bool,
    pub acoustid_api_key: String,
    pub musicbrainz_user_agent: String,
    pub path_templates: PathTemplateConfig,
    pub in_place: InPlaceConfig,
    pub metadata_fields: MetadataFields,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PathTemplateConfig {
    pub default_template: String,
    pub compilation_template: String,
    pub unknown_artist: String,
    pub unknown_album: String,
    pub unknown_title: String,
    pub track_padding: usize,
    pub disc_padding: usize,
    pub max_filename_length: usize,
    pub collision_strategy: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct InPlaceConfig {
    pub write_tags: bool,
    pub embed_cover_art: bool,
    pub rename_files: bool,
    pub rename_folders: bool,
    pub filename_template: String,
    pub preserve_mtime: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MetadataFields {
    pub title: bool,
    pub artist: bool,
    pub album: bool,
    pub album_artist: bool,
    pub track_number: bool,
    pub track_total: bool,
    pub disc_number: bool,
    pub disc_total: bool,
    pub release_date: bool,
    pub genre: bool,
    pub composer: bool,
    pub label: bool,
    pub comment: bool,
    pub isrc: bool,
    pub musicbrainz_recording_id: bool,
    pub musicbrainz_release_id: bool,
    pub musicbrainz_artist_id: bool,
    pub musicbrainz_album_artist_id: bool,
    pub embed_cover_art: bool,
    pub replace_existing_cover_art: bool,
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = env::var("UNUNKNOWN_CONFIG").unwrap_or_else(|_| "config.toml".into());
        let mut value = if Path::new(&path).exists() {
            toml::from_str(&fs::read_to_string(&path).context("read config")?)?
        } else {
            Self::default()
        };
        value.db_path = env::var("UNUNKNOWN_DB").unwrap_or(value.db_path);
        value.input_dir = env::var("UNUNKNOWN_INPUT_DIR").unwrap_or(value.input_dir);
        value.output_dir = env::var("UNUNKNOWN_OUTPUT_DIR").unwrap_or(value.output_dir);
        value.acoustid_api_key =
            env::var("UNUNKNOWN_ACOUSTID_API_KEY").unwrap_or(value.acoustid_api_key);
        value.musicbrainz_user_agent =
            env::var("UNUNKNOWN_MUSICBRAINZ_USER_AGENT").unwrap_or(value.musicbrainz_user_agent);
        Ok(value)
    }
    pub fn public(&self) -> PublicSettings {
        PublicSettings {
            config: self.clone().without_secrets(),
            acoustid_configured: !self.acoustid_api_key.is_empty(),
        }
    }
    fn without_secrets(mut self) -> Self {
        self.acoustid_api_key.clear();
        self
    }
}

#[derive(Serialize)]
pub struct PublicSettings {
    #[serde(flatten)]
    pub config: Config,
    pub acoustid_configured: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db_path: "./cache/ununknown.sqlite".into(),
            input_dir: "./music/input".into(),
            output_dir: "./music/output".into(),
            output_mode: "copy".into(),
            automation_mode: "safe".into(),
            confidence_threshold: 90.0,
            cover_art_enabled: true,
            overwrite_existing_tags: true,
            acoustid_api_key: String::new(),
            musicbrainz_user_agent: "Ununknown/0.1 (configure-your-contact)".into(),
            path_templates: Default::default(),
            in_place: Default::default(),
            metadata_fields: Default::default(),
        }
    }
}
impl Default for PathTemplateConfig {
    fn default() -> Self {
        Self {
            default_template: "$albumartist/$album/$track - $title".into(),
            compilation_template: "Compilations/$album/$track - $artist - $title".into(),
            unknown_artist: "Unknown Artist".into(),
            unknown_album: "Unknown Album".into(),
            unknown_title: "Unknown Title".into(),
            track_padding: 2,
            disc_padding: 2,
            max_filename_length: 255,
            collision_strategy: "skip".into(),
        }
    }
}
impl Default for InPlaceConfig {
    fn default() -> Self {
        Self {
            write_tags: true,
            embed_cover_art: true,
            rename_files: false,
            rename_folders: false,
            filename_template: "$track - $title".into(),
            preserve_mtime: false,
        }
    }
}
impl Default for MetadataFields {
    fn default() -> Self {
        Self {
            title: true,
            artist: true,
            album: true,
            album_artist: true,
            track_number: true,
            track_total: true,
            disc_number: true,
            disc_total: true,
            release_date: true,
            genre: true,
            composer: false,
            label: false,
            comment: false,
            isrc: true,
            musicbrainz_recording_id: true,
            musicbrainz_release_id: true,
            musicbrainz_artist_id: true,
            musicbrainz_album_artist_id: true,
            embed_cover_art: true,
            replace_existing_cover_art: false,
        }
    }
}
