mod resource;

use kube::CustomResourceExt;
use crate::resource::RotateKeys;

fn main() {
    print!(
        "{}",
        serde_yaml::to_string(&RotateKeys::crd()).unwrap()
    )
}
