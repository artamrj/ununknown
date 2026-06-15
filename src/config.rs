use anyhow::Result;
use serde::{Deserialize, Serialize};

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
    pub expert_mode: bool,
    #[serde(skip)]
    pub acoustid_api_key: String,
    pub workspace_retention_days: u32,
    pub job_retention_days: u32,
    #[serde(skip)]
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
    pub fn public(&self) -> PublicSettings {
        PublicSettings {
            config: self.clone().without_secrets(),
            acoustid_configured: !self.acoustid_api_key.is_empty(),
            musicbrainz_configured: Self::valid_musicbrainz_user_agent(
                &self.musicbrainz_user_agent,
            ),
        }
    }
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.input_dir.trim().is_empty(),
            "Input folder is required"
        );
        anyhow::ensure!(
            self.output_mode == "in_place" || !self.output_dir.trim().is_empty(),
            "Output folder is required in copy mode"
        );
        anyhow::ensure!(
            matches!(self.output_mode.as_str(), "copy" | "in_place"),
            "Output mode must be copy or in_place"
        );
        anyhow::ensure!(
            matches!(
                self.automation_mode.as_str(),
                "safe" | "aggressive" | "manual" | "custom"
            ),
            "Automation mode must be safe, aggressive, manual, or custom"
        );
        anyhow::ensure!(
            (0.0..=100.0).contains(&self.confidence_threshold),
            "Confidence threshold must be between 0 and 100"
        );
        anyhow::ensure!(
            Self::valid_musicbrainz_user_agent(&self.musicbrainz_user_agent),
            "MusicBrainz contact must include an email address or website, for example: Ununknown/0.1 (you@example.com)"
        );
        anyhow::ensure!(
            !self.path_templates.default_template.trim().is_empty(),
            "Output template is required"
        );
        anyhow::ensure!(
            matches!(
                self.path_templates.collision_strategy.as_str(),
                "skip" | "overwrite" | "rename"
            ),
            "Collision behavior must be skip, overwrite, or rename"
        );
        anyhow::ensure!(
            self.path_templates.track_padding <= 8 && self.path_templates.disc_padding <= 8,
            "Number padding cannot exceed 8"
        );
        anyhow::ensure!(
            (32..=255).contains(&self.path_templates.max_filename_length),
            "Filename limit must be between 32 and 255"
        );
        anyhow::ensure!(
            (1..=365).contains(&self.workspace_retention_days)
                && (1..=365).contains(&self.job_retention_days),
            "Retention must be between 1 and 365 days"
        );
        let destructive = self.output_mode == "in_place"
            || self.in_place.rename_files
            || self.in_place.rename_folders
            || self.path_templates.collision_strategy == "overwrite"
            || self.metadata_fields.replace_existing_cover_art;
        anyhow::ensure!(
            self.expert_mode || !destructive,
            "Enable Expert Mode before using in-place, rename, overwrite, or cover replacement settings"
        );
        Ok(())
    }
    fn without_secrets(mut self) -> Self {
        self.acoustid_api_key.clear();
        self
    }
    pub fn valid_musicbrainz_user_agent(value: &str) -> bool {
        let value = value.trim();
        value.contains('/')
            && value.contains('(')
            && value.contains(')')
            && (value.contains('@') || value.contains("http"))
    }
}

#[derive(Serialize)]
pub struct PublicSettings {
    #[serde(flatten)]
    pub config: Config,
    pub acoustid_configured: bool,
    pub musicbrainz_configured: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db_path: "/cache/ununknown.sqlite".into(),
            input_dir: "/music/input".into(),
            output_dir: "/music/output".into(),
            output_mode: "copy".into(),
            automation_mode: "safe".into(),
            confidence_threshold: 90.0,
            cover_art_enabled: true,
            overwrite_existing_tags: true,
            expert_mode: false,
            acoustid_api_key: String::new(),
            workspace_retention_days: 7,
            job_retention_days: 7,
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
