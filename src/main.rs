mod key_set;
mod resource;

use base64::alphabet::URL_SAFE;
use base64::engine::{Engine, GeneralPurpose, general_purpose::NO_PAD};
use chrono::Utc;
use futures::StreamExt;
use k8s_openapi::api::core::v1::ObjectReference;
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use kube::runtime::controller::Action;
use kube::runtime::events::{Recorder, Reporter};
use kube::runtime::watcher;
use kube::{Api, Client, Resource};
use kube_core::object::HasStatus;
use kube_operator_util::status::{is_not_ready, patch_status};
use kube_operator_util::util::{
    error_event, is_own_update, report_reconciliation, serial_controller, should_reconcile,
    simple_post_params,
};
use log::info;
use openssl::error::ErrorStack;
use openssl::rsa::Rsa;
use resource::{RotateKeys, Target};
use rust_string_utils::replace;
use rustls::crypto::ring::default_provider;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::cmp;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Debug;
use std::string::FromUtf8Error;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::time::sleep;
use uuid::Uuid;

const ALG_ANCHOR: &str = "{ALG}";
const BACK_OFF: Duration = Duration::from_secs(5);
const CONTROLLER: &str = "rotatekeys.pincette.net";
const CUSTOM_ENGINE: GeneralPurpose = GeneralPurpose::new(&URL_SAFE, NO_PAD);
const KID_ANCHOR: &str = "{KID}";
const PEM_ANCHOR: &str = "{PEM}";
const VERSION: &str = "1.0.0";

struct Data {
    api: Api<RotateKeys>,
    client: Client,
    recorder: Recorder,
}

#[derive(Clone)]
struct Key {
    alg: String,
    e: String,
    kid: String,
    n: String,
    pem: String,
}

#[derive(Error, Debug)]
enum OperatorError {
    #[error("the config map could not be created: {0}")]
    ConfigMapCreation(String),
    #[error("key set serialisation error: {0}")]
    KeySet(#[from] serde_json::Error),
    #[error("kube API error: {0}")]
    Kube(#[from] kube::Error),
    #[error("RSA error: {0}")]
    Rsa(#[from] ErrorStack),
    #[error("the secret could not be created: {0}")]
    SecretCreation(String),
    #[error("UTF-8 error: {0}")]
    Utf8(#[from] FromUtf8Error),
}

fn add_at_most<T>(s: &[T], v: T, at_most: usize) -> Vec<T>
where
    T: Clone,
{
    last_elements(s, at_most - 1)
        .iter()
        .chain(&[v])
        .cloned()
        .collect()
}

async fn child_changed(obj: &RotateKeys, ctx: &Data) -> Result<bool, OperatorError> {
    let mut result = false;

    for t in obj.spec.targets.as_slice() {
        result |= secret_changed(t, &ctx.client).await?;
        result |= config_map_changed(t, &ctx.client).await?;
    }

    Ok(result)
}

async fn config_map_changed(target: &Target, client: &Client) -> Result<bool, OperatorError> {
    Ok(
        Api::<ConfigMap>::namespaced(client.clone(), &target.namespace)
            .get_opt(&target.name)
            .await?
            .is_none_or(|s| !is_own_update(&s.metadata, CONTROLLER)),
    )
}

fn context(api: &Api<RotateKeys>, client: &Client) -> Arc<Data> {
    Arc::new(Data {
        api: api.clone(),
        client: client.clone(),
        recorder: Recorder::new(
            client.clone(),
            Reporter {
                controller: CONTROLLER.to_string(),
                instance: None,
            },
        ),
    })
}

fn error_policy(_object: Arc<RotateKeys>, _err: &OperatorError, _ctx: Arc<Data>) -> Action {
    Action::requeue(Duration::from_secs(5))
}

fn generate_key() -> Result<Key, OperatorError> {
    let rsa = Rsa::generate(2048)?;

    Ok(Key {
        alg: "RS512".to_string(),
        e: CUSTOM_ENGINE.encode(rsa.e().to_vec()),
        kid: Uuid::new_v4().to_string(),
        n: CUSTOM_ENGINE.encode(rsa.n().to_vec()),
        pem: String::from_utf8(rsa.private_key_to_pem()?)?.replace('\n', "\\n"),
    })
}

fn is_expired(obj: &RotateKeys) -> bool {
    obj.status
        .clone()
        .and_then(|s| s.last_success())
        .map(|s| s + Duration::from_secs(obj.spec.interval_seconds as u64) < Utc::now())
        .unwrap_or(true)
}

fn last_elements<T>(s: &[T], at_most: usize) -> &[T] {
    if s.len() <= at_most {
        s
    } else {
        &s[cmp::max(0, s.len() - at_most)..s.len()]
    }
}

#[tokio::main]
async fn main() -> Result<(), OperatorError> {
    env_logger::init();
    info!("Version: {VERSION}");
    default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let client = Client::try_default().await?;
    let config_maps = Api::<ConfigMap>::all(client.clone());
    let rotate_keys = Api::<RotateKeys>::all(client.clone());
    let secrets = Api::<Secret>::all(client.clone());

    serial_controller(&rotate_keys)
        .owns(secrets, watcher::Config::default())
        .owns(config_maps, watcher::Config::default())
        .run(reconcile, error_policy, context(&rotate_keys, &client))
        .for_each(|res| async { report_reconciliation(res) })
        .await;

    Ok(())
}

fn owner_references(obj: &RotateKeys) -> Vec<OwnerReference> {
    Vec::from(&*obj.owner_ref(&()).into_iter().collect::<Vec<_>>())
}

async fn patch_child<T>(target: &Target, api: &Api<T>, resource: &T) -> kube::Result<T>
where
    T: Serialize + Debug + Clone + DeserializeOwned,
{
    let params = simple_post_params(CONTROLLER);

    match api.get_opt(&target.name).await? {
        Some(_) => api.replace(&target.name, &params, resource).await,
        None => api.create(&params, resource).await,
    }
}

async fn patch_config_map<'a>(
    obj: &'a RotateKeys,
    target: &Target,
    key: &Key,
    client: &Client,
) -> Result<&'a RotateKeys, OperatorError> {
    let api = Api::<ConfigMap>::namespaced(client.clone(), &target.namespace);
    let key_set = update_keys(obj, &api, target, key).await?;
    let key_set_string = serde_json::to_string(&key_set)?;
    let mut keys_map = BTreeMap::new();

    keys_map.insert(target.config_map_field.clone(), key_set_string);

    let config_map = ConfigMap {
        metadata: ObjectMeta {
            name: Some(target.name.clone()),
            namespace: Some(target.namespace.clone()),
            owner_references: Some(owner_references(obj)),
            ..ObjectMeta::default()
        },
        data: Some(keys_map),
        ..Default::default()
    };

    info!(
        "Updating config map {}",
        &config_map.metadata.name.as_ref().unwrap()
    );

    patch_child(target, &api, &config_map)
        .await
        .map_err(|e| OperatorError::ConfigMapCreation(source_message(&e)))?;

    Ok(obj)
}

async fn patch_resources<'a>(
    obj: &'a RotateKeys,
    ctx: &Data,
) -> Result<&'a RotateKeys, OperatorError> {
    let key = generate_key()?;

    for t in obj.spec.targets.as_slice() {
        patch_secret(obj, t, &key, &ctx.client).await?;
        patch_config_map(obj, t, &key, &ctx.client).await?;
    }

    Ok(obj)
}

async fn patch_secret<'a>(
    obj: &'a RotateKeys,
    target: &Target,
    key: &Key,
    client: &Client,
) -> Result<&'a RotateKeys, OperatorError> {
    let api = Api::<Secret>::namespaced(client.clone(), &target.namespace);
    let secret = Secret {
        string_data: Some(secret_data(target, key)),
        metadata: ObjectMeta {
            name: Some(target.name.clone()),
            namespace: Some(target.namespace.clone()),
            owner_references: Some(owner_references(obj)),
            ..ObjectMeta::default()
        },
        type_: Some("Opaque".to_string()),
        ..Default::default()
    };

    info!(
        "Updating secret {}",
        &secret.metadata.name.as_ref().unwrap()
    );

    patch_child(target, &api, &secret)
        .await
        .map_err(|e| OperatorError::SecretCreation(source_message(&e)))?;

    info!("Serving key {0}", key.kid);
    Ok(obj)
}

async fn reconcile(obj: Arc<RotateKeys>, ctx: Arc<Data>) -> Result<Action, OperatorError> {
    if is_not_ready(obj.status()) {
        sleep(BACK_OFF).await;
    }

    if should_reconcile(obj.as_ref(), CONTROLLER)
        || child_changed(&obj, &ctx).await?
        || is_expired(&obj)
    {
        reconciliation_result(
            obj.clone(),
            &ctx,
            reconcile_action(&obj, &ctx).await,
            &obj.object_ref(&()),
        )
        .await
    } else {
        Ok(requeue(&obj))
    }
}

async fn reconcile_action(obj: &RotateKeys, ctx: &Data) -> Result<Action, OperatorError> {
    patch_resources(obj, ctx).await?;
    Ok(requeue(obj))
}

async fn reconciliation_result(
    obj: Arc<RotateKeys>,
    ctx: &Data,
    result: Result<Action, OperatorError>,
    obj_ref: &ObjectReference,
) -> Result<Action, OperatorError> {
    match result {
        Err(e) => {
            patch_status(&ctx.api, &obj, Some(&e.to_string()), CONTROLLER).await?;
            ctx.recorder
                .publish(&error_event(&e.to_string(), "update"), obj_ref)
                .await?;
            Err(e)
        }
        Ok(r) => {
            patch_status(&ctx.api, &obj, None, CONTROLLER).await?;
            Ok(r)
        }
    }
}

fn replace_anchors(value: &str, key: &Key) -> String {
    replace(
        &replace(
            &replace(&value.to_string(), &KID_ANCHOR.to_string(), &key.kid),
            &PEM_ANCHOR.to_string(),
            &key.pem,
        ),
        &ALG_ANCHOR.to_string(),
        &key.alg,
    )
}

fn requeue(obj: &RotateKeys) -> Action {
    Action::requeue(Duration::from_secs(obj.spec.interval_seconds as u64))
}

async fn secret_changed(target: &Target, client: &Client) -> Result<bool, OperatorError> {
    Ok(Api::<Secret>::namespaced(client.clone(), &target.namespace)
        .get_opt(&target.name)
        .await?
        .is_none_or(|s| !is_own_update(&s.metadata, CONTROLLER)))
}

fn secret_data(target: &Target, key: &Key) -> BTreeMap<String, String> {
    target
        .template
        .iter()
        .map(|(k, v)| (k.clone(), replace_anchors(v, key)))
        .collect()
}

fn source_message(error: &dyn Error) -> String {
    error.source().map_or(error.to_string(), |s| s.to_string())
}

fn to_public_key(key: &Key) -> key_set::PublicKey {
    key_set::PublicKey {
        alg: key_set::PublicKey::default_alg(),
        e: key.e.clone(),
        kid: key.kid.clone(),
        kty: key_set::PublicKey::default_kty(),
        n: key.n.clone(),
        use_field: key_set::PublicKey::default_use(),
    }
}

async fn update_keys(
    obj: &RotateKeys,
    api: &Api<ConfigMap>,
    target: &Target,
    key: &Key,
) -> Result<key_set::KeySet, OperatorError> {
    let config_map = api.get_opt(&target.name).await?;
    let key_set: key_set::KeySet = config_map
        .and_then(|m| m.data)
        .and_then(|d| d.get(&target.config_map_field).cloned())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| key_set::KeySet { keys: Vec::new() });

    Ok(key_set::KeySet {
        keys: add_at_most(&key_set.keys, to_public_key(key), obj.spec.depth as usize),
    })
}
