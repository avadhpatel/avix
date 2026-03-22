use crate::error::AvixError;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modality {
    Text,
    Image,
    Speech,
    Transcription,
    Embedding,
}

impl Modality {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Speech => "speech",
            Self::Transcription => "transcription",
            Self::Embedding => "embedding",
        }
    }

    pub fn all() -> &'static [Modality] {
        &[
            Modality::Text,
            Modality::Image,
            Modality::Speech,
            Modality::Transcription,
            Modality::Embedding,
        ]
    }
}

impl FromStr for Modality {
    type Err = AvixError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "text" => Ok(Modality::Text),
            "image" => Ok(Modality::Image),
            "speech" => Ok(Modality::Speech),
            "transcription" => Ok(Modality::Transcription),
            "embedding" => Ok(Modality::Embedding),
            other => Err(AvixError::ConfigParse(format!("unknown modality: {other}"))),
        }
    }
}
