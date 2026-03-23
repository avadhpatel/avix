use serde::{Deserialize, Serialize};

/// The typed payload for a SIGPIPE signal (§6.3).
/// `text` is injected into the agent's LLM context immediately.
/// `attachments` is optional and currently parsed + validated but not yet
/// injected — RuntimeExecutor ignores it until multimodal support lands.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SigPipePayload {
    /// Plain-text instruction or message injected into the agent's context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// Optional file/data attachments. Ignored by RuntimeExecutor until
    /// multimodal injection is implemented (see future atp-gap-H).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<PipeAttachment>>,
}

/// A single attachment carried inside a SIGPIPE payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PipeAttachment {
    /// Binary or text data embedded directly in the message, base64-encoded.
    Inline {
        /// MIME type, e.g. "image/png", "text/plain", "application/pdf".
        content_type: String,
        /// Must be "base64". Reserved for future encodings.
        encoding: InlineEncoding,
        /// The encoded data string.
        data: String,
        /// Optional human-readable label shown in UIs.
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// A reference to a file already present in the VFS.
    /// RuntimeExecutor reads the file at injection time.
    VfsRef {
        /// Absolute VFS path, e.g. "/users/alice/report.pdf".
        path: String,
        /// MIME type hint for the LLM multimodal adapter.
        content_type: String,
        /// Optional human-readable label.
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InlineEncoding {
    Base64,
}

impl SigPipePayload {
    /// Validate the payload before forwarding to the kernel.
    /// Returns Err with a human-readable message if invalid.
    pub fn validate(&self) -> Result<(), String> {
        if self.text.is_none() && self.attachments.as_ref().is_none_or(Vec::is_empty) {
            return Err("SIGPIPE payload must have 'text' or at least one attachment".into());
        }
        if let Some(attachments) = &self.attachments {
            for (i, att) in attachments.iter().enumerate() {
                match att {
                    PipeAttachment::Inline { data, encoding, .. } => {
                        if *encoding == InlineEncoding::Base64 {
                            use base64::Engine;
                            base64::engine::general_purpose::STANDARD
                                .decode(data)
                                .map_err(|e| format!("attachment[{i}] invalid base64: {e}"))?;
                        }
                    }
                    PipeAttachment::VfsRef { path, .. } => {
                        if !path.starts_with('/') {
                            return Err(format!(
                                "attachment[{i}] vfs_ref path must be absolute, got '{path}'"
                            ));
                        }
                        if path.starts_with("/secrets/") {
                            return Err(format!(
                                "attachment[{i}] vfs_ref path '/secrets/' is forbidden"
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_only_payload_is_valid() {
        let p = SigPipePayload {
            text: Some("hello".into()),
            attachments: None,
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn empty_payload_is_invalid() {
        let p = SigPipePayload::default();
        assert!(p.validate().is_err());
    }

    #[test]
    fn valid_base64_inline_attachment_passes() {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"hello world");
        let p = SigPipePayload {
            text: None,
            attachments: Some(vec![PipeAttachment::Inline {
                content_type: "text/plain".into(),
                encoding: InlineEncoding::Base64,
                data,
                label: None,
            }]),
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn invalid_base64_inline_attachment_fails() {
        let p = SigPipePayload {
            text: None,
            attachments: Some(vec![PipeAttachment::Inline {
                content_type: "image/png".into(),
                encoding: InlineEncoding::Base64,
                data: "not!!valid%%base64".into(),
                label: None,
            }]),
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn vfs_ref_absolute_path_passes() {
        let p = SigPipePayload {
            text: Some("see attached".into()),
            attachments: Some(vec![PipeAttachment::VfsRef {
                path: "/users/alice/report.pdf".into(),
                content_type: "application/pdf".into(),
                label: None,
            }]),
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn vfs_ref_relative_path_fails() {
        let p = SigPipePayload {
            text: None,
            attachments: Some(vec![PipeAttachment::VfsRef {
                path: "users/alice/report.pdf".into(),
                content_type: "application/pdf".into(),
                label: None,
            }]),
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn vfs_ref_secrets_path_is_forbidden() {
        let p = SigPipePayload {
            text: None,
            attachments: Some(vec![PipeAttachment::VfsRef {
                path: "/secrets/api_key".into(),
                content_type: "text/plain".into(),
                label: None,
            }]),
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn payload_round_trips_through_json() {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"img");
        let p = SigPipePayload {
            text: Some("look at this".into()),
            attachments: Some(vec![
                PipeAttachment::Inline {
                    content_type: "image/png".into(),
                    encoding: InlineEncoding::Base64,
                    data: data.clone(),
                    label: Some("screenshot".into()),
                },
                PipeAttachment::VfsRef {
                    path: "/users/alice/doc.pdf".into(),
                    content_type: "application/pdf".into(),
                    label: None,
                },
            ]),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: SigPipePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(back.text.as_deref(), Some("look at this"));
        assert_eq!(back.attachments.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn inline_attachment_type_tag_serializes_correctly() {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"x");
        let att = PipeAttachment::Inline {
            content_type: "image/png".into(),
            encoding: InlineEncoding::Base64,
            data,
            label: None,
        };
        let json = serde_json::to_string(&att).unwrap();
        assert!(json.contains("\"type\":\"inline\""));
    }

    #[test]
    fn vfs_ref_attachment_type_tag_serializes_correctly() {
        let att = PipeAttachment::VfsRef {
            path: "/users/alice/f.txt".into(),
            content_type: "text/plain".into(),
            label: None,
        };
        let json = serde_json::to_string(&att).unwrap();
        assert!(json.contains("\"type\":\"vfs_ref\""));
    }
}
