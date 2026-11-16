//! JSON Schema validation against the schemas in `protocol/schemas`.

use crate::{Error, Result};
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schema {
    ServerInfo,
    GameManifest,
    ReleaseManifest,
    KeyRotation,
}

impl Schema {
    pub fn source(self) -> &'static str {
        match self {
            Schema::ServerInfo => {
                include_str!("../../../protocol/schemas/server-info.schema.json")
            }
            Schema::GameManifest => {
                include_str!("../../../protocol/schemas/game-manifest.schema.json")
            }
            Schema::ReleaseManifest => {
                include_str!("../../../protocol/schemas/release-manifest.schema.json")
            }
            Schema::KeyRotation => {
                include_str!("../../../protocol/schemas/key-rotation.schema.json")
            }
        }
    }

    fn validator(self) -> &'static jsonschema::Validator {
        static CELLS: [OnceLock<jsonschema::Validator>; 4] = [
            OnceLock::new(),
            OnceLock::new(),
            OnceLock::new(),
            OnceLock::new(),
        ];
        CELLS[self as usize].get_or_init(|| {
            let schema: Value =
                serde_json::from_str(self.source()).expect("embedded schema is valid JSON");
            jsonschema::validator_for(&schema).expect("embedded schema compiles")
        })
    }
}

/// Validate `instance`; the error lists every violation with its JSON pointer.
pub fn validate(schema: Schema, instance: &Value) -> Result<()> {
    let errors: Vec<String> = schema
        .validator()
        .iter_errors(instance)
        .map(|e| {
            let at = e.instance_path.to_string();
            if at.is_empty() {
                e.to_string()
            } else {
                format!("{at}: {e}")
            }
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::Schema(errors.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../protocol/fixtures")
            .join(name);
        serde_json::from_str(
            &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
        )
        .unwrap()
    }

    #[test]
    fn valid_fixtures_pass() {
        validate(Schema::ServerInfo, &fixture("server-info.json")).unwrap();
        validate(Schema::GameManifest, &fixture("game-manifest.json")).unwrap();
        validate(Schema::ReleaseManifest, &fixture("release-manifest.json")).unwrap();
        validate(Schema::KeyRotation, &fixture("key-rotation.json")).unwrap();
    }

    #[test]
    fn invalid_fixtures_fail() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../protocol/fixtures/invalid");
        let mut seen = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            let schema = if name.starts_with("server-info") {
                Schema::ServerInfo
            } else if name.starts_with("game-manifest") {
                Schema::GameManifest
            } else if name.starts_with("release-manifest") {
                Schema::ReleaseManifest
            } else {
                Schema::KeyRotation
            };
            let value: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert!(
                validate(schema, &value).is_err(),
                "{name} should be rejected"
            );
            seen += 1;
        }
        assert!(seen >= 6, "expected invalid fixtures, found {seen}");
    }

    #[test]
    fn error_names_the_location() {
        let mut v = fixture("release-manifest.json");
        v["platforms"]["linux-x86_64"]["files"][0]["chunks"][0]["size"] = serde_json::json!(0);
        let err = validate(Schema::ReleaseManifest, &v)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("/platforms/linux-x86_64/files/0/chunks/0/size"),
            "{err}"
        );
    }
}
