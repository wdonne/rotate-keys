# Rotate Keys Operator

With this Kubernetes operator you can generate public/private key pairs in a rotating fashion. With a cluster-scoped custom resource you declare the rotation interval and the targets in the one or more namespaces, which all receive the same keys. For each target a secret and a configmap are generated. The secret contains the PEM-encoded private key and the configmap contains a JSON Web Key Set with the latest key that are in circulation. The depth of this is configurable.

Currently, the operator only generates 2048 bit RSA keys. The JWKS sets the algorithm to RS512, which suggests that messages should be signed with SHA512, but this can be changed by the consumer of the keys. Optionally, you can also set this algorithm in the secret.

A custom resource looks like this:

```
apiVersion: pincette.net/v1
kind: RotateKeys
metadata:
  name: test
spec:
  depth: 5
  intervalSeconds: 3600
  targets:
    - name: test
      namespace: test1
      configMapField: keys.json
      template:
        alg: "{ALG}"
        kid: "{KID}"
        pem: "{PEM}"
    - name: test
      namespace: test2
      template:
        key.json: >-
          {"alg": "{ALG}", "kid": "{KID}", "pem": "{PEM}"}
```

The optional fields `depth` and `intervalSeconds` are shown with their default values. For each target, the `name` field is used for both the generated secret and configmap. The optional `configMapField` contains the name of the field that holds the JWKS in the generated configmap. Its default value is `keys.json`. The `template` field is for the private key. It defines the fields that come under the `data` field in the generated secret. The strings with curly braces are placeholders for the substitution with actual values. They are optional, but without PEM it won't be very useful.

Install the operator as follows:

```bash
helm repo add wdonne https://wdonne.github.io/helm
helm repo update
helm install rotate-keys wdonne/rotate-keys --namespace rotate-keys --create-namespace
```
