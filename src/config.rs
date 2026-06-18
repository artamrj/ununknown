use crate::types::{AutomationMode, CollisionStrategy, OutputMode};
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(skip)]
    pub db_path: String,
    pub input_dir: String,
    pub output_dir: String,
    pub output_mode: OutputMode,
    pub automation_mode: AutomationMode,
    pub confidence_threshold: f64,
    pub track_attempts: u32,
    pub scan_worker_concurrency: usize,
    pub metadata_read_concurrency: usize,
    pub fingerprint_concurrency: usize,
    pub acoustid_concurrency: usize,
    pub artwork_download_concurrency: usize,
    pub tag_write_concurrency: usize,
    pub db_write_batch_size: usize,
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
    pub collision_strategy: CollisionStrategy,
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
            self.output_mode == OutputMode::InPlace || !self.output_dir.trim().is_empty(),
            "Output folder is required in copy mode"
        );
        anyhow::ensure!(
            (0.0..=100.0).contains(&self.confidence_threshold),
            "Confidence threshold must be between 0 and 100"
        );
        anyhow::ensure!(
            (1..=10).contains(&self.track_attempts),
            "Track attempts must be between 1 and 10"
        );
        anyhow::ensure!(
            (1..=64).contains(&self.scan_worker_concurrency),
            "Scan workers must be between 1 and 64"
        );
        anyhow::ensure!(
            (1..=16).contains(&self.metadata_read_concurrency),
            "Metadata read workers must be between 1 and 16"
        );
        anyhow::ensure!(
            (1..=8).contains(&self.fingerprint_concurrency),
            "Fingerprint workers must be between 1 and 8"
        );
        anyhow::ensure!(
            (1..=8).contains(&self.acoustid_concurrency),
            "AcoustID lookups must be between 1 and 8"
        );
        anyhow::ensure!(
            (1..=8).contains(&self.artwork_download_concurrency),
            "Artwork downloads must be between 1 and 8"
        );
        anyhow::ensure!(
            (1..=8).contains(&self.tag_write_concurrency),
            "Tag writers must be between 1 and 8"
        );
        anyhow::ensure!(
            (1..=250).contains(&self.db_write_batch_size),
            "DB write batch size must be between 1 and 250"
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
        let destructive = self.output_mode == OutputMode::InPlace
            || self.in_place.rename_files
            || self.in_place.rename_folders
            || self.path_templates.collision_strategy == CollisionStrategy::Overwrite
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
            output_mode: OutputMode::Copy,
            automation_mode: AutomationMode::Safe,
            confidence_threshold: 90.0,
            track_attempts: 3,
            scan_worker_concurrency: 8,
            metadata_read_concurrency: 6,
            fingerprint_concurrency: 3,
            acoustid_concurrency: 3,
            artwork_download_concurrency: 3,
            tag_write_concurrency: 2,
            db_write_batch_size: 25,
            cover_art_enabled: true,
            overwrite_existing_tags: true,
            expert_mode: false,
            acoustid_api_key: String::new(),
            workspace_retention_days: 1,
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
            collision_strategy: CollisionStrategy::Skip,
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

#[cfg(test)]
mod tests {
    use super::Config;
    use crate::types::{AutomationMode, CollisionStrategy, OutputMode};

    fn valid_config() -> Config {
        Config {
            musicbrainz_user_agent: "Ununknown/0.1 (test@example.com)".into(),
            ..Default::default()
        }
    }

    #[test]
    fn pipeline_concurrency_defaults_are_conservative() {
        let cfg = Config::default();
        assert_eq!(cfg.scan_worker_concurrency, 8);
        assert_eq!(cfg.metadata_read_concurrency, 6);
        assert_eq!(cfg.fingerprint_concurrency, 3);
        assert_eq!(cfg.acoustid_concurrency, 3);
        assert_eq!(cfg.artwork_download_concurrency, 3);
        assert_eq!(cfg.tag_write_concurrency, 2);
        assert_eq!(cfg.db_write_batch_size, 25);
    }

    #[test]
    fn typed_mode_defaults_match_existing_settings() {
        let cfg = Config::default();
        assert_eq!(cfg.output_mode, OutputMode::Copy);
        assert_eq!(cfg.automation_mode, AutomationMode::Safe);
        assert_eq!(
            cfg.path_templates.collision_strategy,
            CollisionStrategy::Skip
        );
        assert_eq!(
            serde_json::to_value(&cfg).unwrap()["output_mode"],
            serde_json::json!("copy")
        );
        assert_eq!(
            serde_json::to_value(&cfg).unwrap()["automation_mode"],
            serde_json::json!("safe")
        );
        assert_eq!(
            serde_json::to_value(&cfg).unwrap()["path_templates"]["collision_strategy"],
            serde_json::json!("skip")
        );
    }

    #[test]
    fn invalid_typed_modes_are_rejected_by_deserialization() {
        assert!(
            serde_json::from_str::<Config>(r#"{"output_mode":"move","automation_mode":"safe"}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<Config>(
                r#"{"output_mode":"copy","automation_mode":"reckless"}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<Config>(r#"{"path_templates":{"collision_strategy":"merge"}}"#)
                .is_err()
        );
    }

    #[test]
    fn pipeline_concurrency_validation_accepts_documented_ranges() {
        let mut cfg = valid_config();
        cfg.scan_worker_concurrency = 64;
        cfg.metadata_read_concurrency = 16;
        cfg.fingerprint_concurrency = 8;
        cfg.acoustid_concurrency = 8;
        cfg.artwork_download_concurrency = 8;
        cfg.tag_write_concurrency = 8;
        cfg.db_write_batch_size = 250;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn pipeline_concurrency_validation_rejects_out_of_range_values() {
        for invalid in [
            ("scan", 0, 6, 3, 3, 3, 2, 25),
            ("scan", 65, 6, 3, 3, 3, 2, 25),
            ("metadata", 8, 0, 3, 3, 3, 2, 25),
            ("metadata", 8, 17, 3, 3, 3, 2, 25),
            ("fingerprint", 8, 6, 0, 3, 3, 2, 25),
            ("fingerprint", 8, 6, 9, 3, 3, 2, 25),
            ("acoustid", 8, 6, 3, 0, 3, 2, 25),
            ("acoustid", 8, 6, 3, 9, 3, 2, 25),
            ("artwork", 8, 6, 3, 3, 0, 2, 25),
            ("artwork", 8, 6, 3, 3, 9, 2, 25),
            ("tag", 8, 6, 3, 3, 3, 0, 25),
            ("tag", 8, 6, 3, 3, 3, 9, 25),
            ("db", 8, 6, 3, 3, 3, 2, 0),
            ("db", 8, 6, 3, 3, 3, 2, 251),
        ] {
            let (name, scan, metadata, fingerprint, acoustid, artwork, tag, db_batch) = invalid;
            let mut cfg = valid_config();
            cfg.scan_worker_concurrency = scan;
            cfg.metadata_read_concurrency = metadata;
            cfg.fingerprint_concurrency = fingerprint;
            cfg.acoustid_concurrency = acoustid;
            cfg.artwork_download_concurrency = artwork;
            cfg.tag_write_concurrency = tag;
            cfg.db_write_batch_size = db_batch;
            assert!(cfg.validate().is_err(), "{name} should be rejected");
        }
    }

    #[test]
    fn missing_json_pipeline_concurrency_uses_defaults() {
        let cfg: Config = serde_json::from_str(
            r#"{
                "input_dir": "/music/input",
                "output_dir": "/music/output",
                "output_mode": "copy",
                "automation_mode": "safe",
                "confidence_threshold": 90.0,
                "track_attempts": 3
            }"#,
        )
        .unwrap();
        assert_eq!(cfg.scan_worker_concurrency, 8);
        assert_eq!(cfg.metadata_read_concurrency, 6);
        assert_eq!(cfg.fingerprint_concurrency, 3);
        assert_eq!(cfg.acoustid_concurrency, 3);
        assert_eq!(cfg.artwork_download_concurrency, 3);
        assert_eq!(cfg.tag_write_concurrency, 2);
        assert_eq!(cfg.db_write_batch_size, 25);
    }
}
