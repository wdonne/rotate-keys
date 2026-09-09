use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct KeySet {
    pub(crate) keys: Vec<PublicKey>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PublicKey {
    #[serde(default = "PublicKey::default_alg")]
    pub(crate) alg: String,
    pub(crate) e: String,
    pub(crate) kid: String,
    #[serde(default = "PublicKey::default_kty")]
    pub(crate) kty: String,
    pub(crate) n: String,
    #[serde(rename = "use", default = "PublicKey::default_use")]
    pub(crate) use_field: String,
}

impl PublicKey {
    pub(crate) fn default_alg() -> String {
        "RS512".to_string()
    }

    pub(crate) fn default_kty() -> String {
        "RSA".to_string()
    }

    pub(crate) fn default_use() -> String {
        "sig".to_string()
    }
}
