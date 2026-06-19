use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};
use uuid::Uuid;

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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    #[default]
    Copy,
    InPlace,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationMode {
    #[default]
    Safe,
    Aggressive,
    Manual,
    Custom,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompilationPreference {
    #[default]
    Avoid,
    Allow,
    Prefer,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchingStrategy {
    Safe,
    #[default]
    Balanced,
    Aggressive,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderMode {
    #[default]
    Primary,
    Fallback,
    Parallel,
    EnrichmentOnly,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Connected,
    MissingApiKey,
    RateLimited,
    Error,
    #[default]
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollisionStrategy {
    #[default]
    Skip,
    Overwrite,
    Rename,
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

impl TrackStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Ready => "ready",
            Self::Review => "review",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for TrackStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TrackStage {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "discovered" => Ok(Self::Discovered),
            "ready" => Ok(Self::Ready),
            "review" => Ok(Self::Review),
            "skipped" => Ok(Self::Skipped),
            "failed" => Ok(Self::Failed),
            _ => Err(format!("unknown track stage: {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateAction {
    #[default]
    None,
    Keep,
    SkipDuplicate,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, sqlx::Type)]
#[sqlx(transparent)]
pub struct TrackId(pub i64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, sqlx::Type)]
#[sqlx(transparent)]
pub struct CandidateId(pub i64);

macro_rules! numeric_id_serde {
    ($ty:ty) => {
        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_i64(self.0)
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Ok(Self(i64::deserialize(deserializer)?))
            }
        }
    };
}

numeric_id_serde!(TrackId);
numeric_id_serde!(CandidateId);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct JobId(pub Uuid);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PreviewToken(pub Uuid);

impl JobId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl PreviewToken {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

macro_rules! uuid_id {
    ($ty:ty) => {
        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $ty {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Ok(Self(Uuid::parse_str(value)?))
            }
        }

        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Ok(Self(
                    Uuid::parse_str(&value).map_err(serde::de::Error::custom)?,
                ))
            }
        }
    };
}

uuid_id!(JobId);
uuid_id!(PreviewToken);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_use_existing_wire_values() {
        assert_eq!(
            serde_json::to_string(&WorkflowPhase::Idle).unwrap(),
            "\"idle\""
        );
        assert_eq!(
            serde_json::to_string(&OutputMode::InPlace).unwrap(),
            "\"in_place\""
        );
        assert_eq!(
            serde_json::to_string(&AutomationMode::Aggressive).unwrap(),
            "\"aggressive\""
        );
        assert_eq!(
            serde_json::to_string(&CompilationPreference::Avoid).unwrap(),
            "\"avoid\""
        );
        assert_eq!(
            serde_json::to_string(&MatchingStrategy::Balanced).unwrap(),
            "\"balanced\""
        );
        assert_eq!(
            serde_json::to_string(&ProviderMode::EnrichmentOnly).unwrap(),
            "\"enrichment_only\""
        );
        assert_eq!(
            serde_json::to_string(&ProviderStatus::MissingApiKey).unwrap(),
            "\"missing_api_key\""
        );
        assert_eq!(
            serde_json::to_string(&CollisionStrategy::Overwrite).unwrap(),
            "\"overwrite\""
        );
        assert_eq!(
            serde_json::to_string(&TrackStage::Ready).unwrap(),
            "\"ready\""
        );
        assert_eq!(
            serde_json::to_string(&DuplicateAction::SkipDuplicate).unwrap(),
            "\"skip_duplicate\""
        );
        assert!(serde_json::from_str::<WorkflowPhase>("\"bogus\"").is_err());
    }

    #[test]
    fn ids_use_existing_wire_shapes() {
        assert_eq!(serde_json::to_string(&TrackId(42)).unwrap(), "42");
        assert_eq!(
            serde_json::from_str::<CandidateId>("7").unwrap(),
            CandidateId(7)
        );
        let token = PreviewToken(Uuid::nil());
        assert_eq!(
            serde_json::to_string(&token).unwrap(),
            "\"00000000-0000-0000-0000-000000000000\""
        );
    }
}
