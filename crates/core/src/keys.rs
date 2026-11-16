//! Ed25519 developer keys and signing of canonical-JSON objects.

use crate::{jcs, Error, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::{fmt, str::FromStr};

const PUB_PREFIX: &str = "ed25519:";
const SECRET_PREFIX: &str = "ed25519-secret:";

/// A developer public key, encoded as `ed25519:<base64url>`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PublicKey(VerifyingKey);

impl PublicKey {
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    /// Verify a detached signature over raw bytes (strict verification).
    pub fn verify_bytes(&self, message: &[u8], sig_b64: &str) -> Result<()> {
        let sig = B64.decode(sig_b64).map_err(|_| Error::BadSignature)?;
        let sig: [u8; 64] = sig.try_into().map_err(|_| Error::BadSignature)?;
        let sig = ed25519_dalek::Signature::from_bytes(&sig);
        self.0
            .verify_strict(message, &sig)
            .map_err(|_| Error::BadSignature)
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{PUB_PREFIX}{}", B64.encode(self.0.as_bytes()))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({self})")
    }
}

impl FromStr for PublicKey {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let bad = |why: &str| Error::invalid("public key", why.to_string());
        let b64 = s
            .strip_prefix(PUB_PREFIX)
            .ok_or_else(|| bad("expected ed25519:<base64url>"))?;
        let bytes = B64.decode(b64).map_err(|_| bad("bad base64url"))?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| bad("must be 32 bytes"))?;
        let key = VerifyingKey::from_bytes(&bytes).map_err(|_| bad("not a valid curve point"))?;
        if key.is_weak() {
            return Err(bad("weak (small-order) key"));
        }
        Ok(PublicKey(key))
    }
}

impl Serialize for PublicKey {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// A developer private key. Never logged; `Debug` is redacted.
pub struct SecretKey(SigningKey);

impl SecretKey {
    pub fn generate() -> Self {
        SecretKey(SigningKey::generate(&mut rand::rngs::OsRng))
    }

    pub fn public(&self) -> PublicKey {
        PublicKey(self.0.verifying_key())
    }

    /// `ed25519-secret:<base64url of the 32-byte seed>` — the on-disk format.
    pub fn to_encoded(&self) -> String {
        format!("{SECRET_PREFIX}{}", B64.encode(self.0.to_bytes()))
    }

    pub fn from_encoded(s: &str) -> Result<Self> {
        let bad = |why: &str| Error::invalid("secret key", why.to_string());
        let b64 = s
            .trim()
            .strip_prefix(SECRET_PREFIX)
            .ok_or_else(|| bad("expected ed25519-secret:<base64url>"))?;
        let bytes = B64.decode(b64).map_err(|_| bad("bad base64url"))?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| bad("must be 32 bytes"))?;
        Ok(SecretKey(SigningKey::from_bytes(&bytes)))
    }

    pub fn sign_bytes(&self, message: &[u8]) -> String {
        B64.encode(self.0.sign(message).to_bytes())
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretKey(<redacted>, public = {})", self.public())
    }
}

fn signing_input(object: &Value) -> Result<String> {
    let map = object
        .as_object()
        .ok_or_else(|| Error::invalid("signed object", "must be a JSON object"))?;
    let mut unsigned = map.clone();
    unsigned.remove("sig");
    jcs::to_canonical_string(&Value::Object(unsigned))
}

/// Sign a JSON object in place: sets `sig` to the signature over the canonical
/// form of the object without `sig`.
pub fn sign_object(key: &SecretKey, object: &mut Value) -> Result<()> {
    let input = signing_input(object)?;
    let sig = key.sign_bytes(input.as_bytes());
    object
        .as_object_mut()
        .expect("checked by signing_input")
        .insert("sig".into(), Value::String(sig));
    Ok(())
}

/// Verify the `sig` member of a JSON object against `key`.
pub fn verify_object(key: &PublicKey, object: &Value) -> Result<()> {
    let sig = object
        .get("sig")
        .and_then(Value::as_str)
        .ok_or(Error::BadSignature)?;
    let input = signing_input(object)?;
    key.verify_bytes(input.as_bytes(), sig)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_encoding_roundtrip() {
        let sk = SecretKey::generate();
        let pk = sk.public();
        let s = pk.to_string();
        assert!(s.starts_with("ed25519:"));
        assert_eq!(s.len(), 8 + 43);
        assert_eq!(s.parse::<PublicKey>().unwrap(), pk);
        let sk2 = SecretKey::from_encoded(&sk.to_encoded()).unwrap();
        assert_eq!(sk2.public(), pk);
        assert!(!format!("{sk:?}").contains(&sk.to_encoded()[15..]));
    }

    #[test]
    fn rejects_bad_keys() {
        assert!("rsa:abc".parse::<PublicKey>().is_err());
        assert!("ed25519:abc".parse::<PublicKey>().is_err());
        // All-zero key is small-order.
        let zero = format!("ed25519:{}", B64.encode([0u8; 32]));
        assert!(zero.parse::<PublicKey>().is_err());
        assert!(SecretKey::from_encoded("ed25519:abc").is_err());
    }

    #[test]
    fn sign_verify_roundtrip() {
        let sk = SecretKey::generate();
        let mut obj = json!({"b": 1, "a": {"y": [1, 2, 3], "x": "héllo"}});
        sign_object(&sk, &mut obj).unwrap();
        verify_object(&sk.public(), &obj).unwrap();

        // Key order and whitespace do not matter.
        let reparsed: Value =
            serde_json::from_str(&serde_json::to_string_pretty(&obj).unwrap()).unwrap();
        verify_object(&sk.public(), &reparsed).unwrap();

        // Any change breaks it.
        let mut tampered = obj.clone();
        tampered["a"]["y"][0] = json!(9);
        assert!(matches!(
            verify_object(&sk.public(), &tampered),
            Err(Error::BadSignature)
        ));
        let mut extra = obj.clone();
        extra["new"] = json!(true);
        assert!(verify_object(&sk.public(), &extra).is_err());

        // Wrong key, missing sig.
        assert!(verify_object(&SecretKey::generate().public(), &obj).is_err());
        let mut unsigned = obj.clone();
        unsigned.as_object_mut().unwrap().remove("sig");
        assert!(verify_object(&sk.public(), &unsigned).is_err());
    }

    #[test]
    fn deterministic_known_vector() {
        // RFC 8032 test vector 1 seed.
        let seed = hex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60");
        let sk = SecretKey::from_encoded(&format!("ed25519-secret:{}", B64.encode(seed))).unwrap();
        assert_eq!(
            sk.public().as_bytes().to_vec(),
            hex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
        );
        assert_eq!(
            sk.public().to_string(),
            "ed25519:11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo"
        );
        let sig = sk.sign_bytes(b"");
        assert_eq!(
            B64.decode(sig).unwrap(),
            hex("e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b")
        );
    }

    fn hex(s: &str) -> Vec<u8> {
        data_encoding::HEXLOWER.decode(s.as_bytes()).unwrap()
    }
}
