use k8s_openapi::serde::{Deserialize, Serialize};
use kube::CustomResource;
use kube_operator_util::status::{GetStatus, Status};
use kube_operator_util::util::GetObjectMeta;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use schemars::JsonSchema;
use std::collections::BTreeMap;

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    kind = "RotateKeys",
    group = "pincette.net",
    version = "v1",
    category = "controllers",
    shortname = "rks",
    printcolumn = r#"{"name":"Health", "type":"string", "jsonPath":".status.health.status"}"#,
    printcolumn = r#"{"name":"Phase", "type":"string", "jsonPath":".status.phase"}"#,
    printcolumn = r#"{"name":"Age", "type":"date", "jsonPath":".metadata.creationTimestamp"}"#
)]
#[kube(status = "Status")]
#[serde(rename_all = "camelCase")]
pub struct RotateKeysSpec {
    #[serde(default = "RotateKeysSpec::default_depth")]
    pub depth: u32,
    #[serde(default = "RotateKeysSpec::default_interval_seconds")]
    pub interval_seconds: u32,
    pub targets: Vec<Target>,
}

impl RotateKeysSpec {
    fn default_depth() -> u32 {
        5
    }

    fn default_interval_seconds() -> u32 {
        3600
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    #[serde(default = "Target::default_config_map_field")]
    pub config_map_field: String,
    pub name: String,
    pub namespace: String,
    pub template: BTreeMap<String, String>,
}

impl GetObjectMeta for RotateKeys {
    fn object_meta(&self) -> &ObjectMeta {
        &self.metadata
    }
}

impl GetStatus for RotateKeys {
    fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }
}

impl Target {
    fn default_config_map_field() -> String {
        "keys.json".to_string()
    }
}
