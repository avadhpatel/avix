use crate::error::AvixError;
use crate::packaging::trust::{TrustStore, TrustedKey};
use tracing::instrument;

pub const AVIX_PUBLIC_KEY: &str = include_str!("../../official-pubkey.asc");

#[derive(Debug)]
pub enum VerifiedBy {
    Official,
    Trusted(TrustedKey),
}

#[instrument]
pub fn verify_signature(
    data: &[u8],
    sig_asc: &str,
    source: &str,
    trust_store: &TrustStore,
) -> Result<VerifiedBy, AvixError> {
    use pgp::composed::{Deserializable, DetachedSignature, SignedPublicKey};

    let (sig, _) = DetachedSignature::from_string(sig_asc)
        .map_err(|e| AvixError::ConfigParse(format!("parse signature: {e}")))?;

    let (official_key, _) = SignedPublicKey::from_string(AVIX_PUBLIC_KEY)
        .map_err(|e| AvixError::ConfigParse(format!("parse official key: {e}")))?;

    if sig.verify(&official_key, data).is_ok() {
        return Ok(VerifiedBy::Official);
    }

    let issuer = sig_issuer_fingerprint(&sig);
    if let Some(fingerprint) = issuer {
        if let Some((key_asc, meta)) = trust_store.get(&fingerprint)? {
            if !meta.allows_source(source) {
                return Err(AvixError::ConfigParse(format!(
                    "key '{}' ({}) is not trusted for source '{}'",
                    meta.label, fingerprint, source,
                )));
            }
            let (pubkey, _) = SignedPublicKey::from_string(&key_asc)
                .map_err(|e| AvixError::ConfigParse(format!("load trusted key: {e}")))?;
            sig.verify(&pubkey, data)
                .map_err(|e| AvixError::ConfigParse(format!("signature invalid: {e}")))?;
            return Ok(VerifiedBy::Trusted(meta));
        }
    }

    Err(AvixError::ConfigParse(
        "signing key is not in the official keyring or trust store — \
         add it with `avix package trust add`"
            .into(),
    ))
}

#[instrument]
fn sig_issuer_fingerprint(sig: &pgp::composed::DetachedSignature) -> Option<String> {
    sig.signature
        .issuer_fingerprint()
        .into_iter()
        .next()
        .map(|fp| hex::encode(fp).to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const TEST_DATA: &[u8] = b"hello world";

    #[test]
    fn verify_unknown_key_errors() {
        let dir = TempDir::new().unwrap();
        let trust_store = TrustStore::new(dir.path());

        let result = verify_signature(TEST_DATA, "invalid sig", "source", &trust_store);
        assert!(result.is_err());
    }

    #[test]
    fn verify_tampered_data_errors() {
        let dir = TempDir::new().unwrap();
        let trust_store = TrustStore::new(dir.path());

        let result = verify_signature(b"tampered data", "", "source", &trust_store);
        assert!(result.is_err());
    }
}
