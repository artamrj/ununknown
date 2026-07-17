use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(skip)]
    pub db_path: String,
    pub input_dir: String,
    pub output_dir: String,
    pub delete_source_after_write: bool,
    pub acoustid_key: String,
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
            acoustid_key: String::new(),
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
