use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(skip)]
    pub db_path: String,
    pub input_dir: String,
    pub output_dir: String,
    pub delete_source_after_write: bool,
    pub automatic_scan_enabled: bool,
    pub automatic_scan_interval_minutes: u64,
    pub acoustid_key: String,
    pub audd_token: String,
    pub spotify_client_id: String,
    pub spotify_client_secret: String,
    pub soundcloud_client_id: String,
    pub soundcloud_client_secret: String,
    pub youtube_api_key: String,
    pub discogs_token: String,
    pub lastfm_key: String,
    pub theaudiodb_key: String,
    pub musicbrainz_user_agent: String,
    pub scan_workers: usize,
    pub fingerprint_workers: usize,
    pub lookup_workers: usize,
    pub write_workers: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db_path: ".local/cache/ununknown.sqlite".into(),
            input_dir: ".local/input".into(),
            output_dir: ".local/output".into(),
            delete_source_after_write: false,
            automatic_scan_enabled: true,
            automatic_scan_interval_minutes: 5,
            acoustid_key: String::new(),
            audd_token: String::new(),
            spotify_client_id: String::new(),
            spotify_client_secret: String::new(),
            soundcloud_client_id: String::new(),
            soundcloud_client_secret: String::new(),
            youtube_api_key: String::new(),
            discogs_token: String::new(),
            lastfm_key: String::new(),
            theaudiodb_key: String::new(),
            musicbrainz_user_agent: "Ununknown/0.6 (https://github.com/artamrj/ununknown)".into(),
            scan_workers: 6,
            fingerprint_workers: 3,
            lookup_workers: 3,
            write_workers: 2,
        }
    }
}

impl Config {
    pub fn normalize(&mut self) {
        self.scan_workers = self.scan_workers.clamp(1, 32);
        self.fingerprint_workers = self.fingerprint_workers.clamp(1, 16);
        self.lookup_workers = self.lookup_workers.clamp(1, 16);
        self.write_workers = self.write_workers.clamp(1, 8);
        self.automatic_scan_interval_minutes =
            self.automatic_scan_interval_minutes.clamp(1, 24 * 60);
    }

    /// Environment values take precedence without being copied back into the
    /// settings database. This lets packaged installations inject credentials
    /// from their process manager or secret store.
    pub fn apply_environment_overrides(&mut self) {
        macro_rules! override_from_env {
            ($field:ident, $name:literal) => {
                if let Ok(value) = std::env::var($name)
                    && !value.trim().is_empty()
                {
                    self.$field = value;
                }
            };
        }

        override_from_env!(input_dir, "UNUNKNOWN_INPUT_DIR");
        override_from_env!(output_dir, "UNUNKNOWN_OUTPUT_DIR");
        override_from_env!(acoustid_key, "UNUNKNOWN_ACOUSTID_KEY");
        override_from_env!(audd_token, "UNUNKNOWN_AUDD_TOKEN");
        override_from_env!(spotify_client_id, "UNUNKNOWN_SPOTIFY_CLIENT_ID");
        override_from_env!(spotify_client_secret, "UNUNKNOWN_SPOTIFY_CLIENT_SECRET");
        override_from_env!(soundcloud_client_id, "UNUNKNOWN_SOUNDCLOUD_CLIENT_ID");
        override_from_env!(
            soundcloud_client_secret,
            "UNUNKNOWN_SOUNDCLOUD_CLIENT_SECRET"
        );
        override_from_env!(youtube_api_key, "UNUNKNOWN_YOUTUBE_API_KEY");
        override_from_env!(discogs_token, "UNUNKNOWN_DISCOGS_TOKEN");
        override_from_env!(lastfm_key, "UNUNKNOWN_LASTFM_KEY");
        override_from_env!(theaudiodb_key, "UNUNKNOWN_THEAUDIODB_KEY");
        self.normalize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_worker_counts_are_bounded() {
        let mut config = Config {
            scan_workers: 0,
            fingerprint_workers: usize::MAX,
            lookup_workers: 0,
            write_workers: usize::MAX,
            ..Config::default()
        };
        config.normalize();
        assert_eq!(config.scan_workers, 1);
        assert_eq!(config.fingerprint_workers, 16);
        assert_eq!(config.lookup_workers, 1);
        assert_eq!(config.write_workers, 8);
    }

    #[test]
    fn automatic_scan_interval_is_bounded() {
        let mut config = Config {
            automatic_scan_interval_minutes: 0,
            ..Config::default()
        };
        config.normalize();
        assert_eq!(config.automatic_scan_interval_minutes, 1);
        config.automatic_scan_interval_minutes = u64::MAX;
        config.normalize();
        assert_eq!(config.automatic_scan_interval_minutes, 24 * 60);
    }
}
