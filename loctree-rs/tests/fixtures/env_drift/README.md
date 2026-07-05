# env_drift fixture (Cut 8 / Lane 4)

Synthesises the Vista April-2026 incident pattern.

- `.env` — fresh local credentials (operator just rotated `DATABASE_URL`).
- `k8s/sealed-secret.yaml` — production SealedSecret with **stale** ciphertext
  for the same `DATABASE_URL`. Tests mtime this file to ~30 days old.
- `k8s/configmap.yaml` — ConfigMap with **mismatching** plain `DATABASE_URL`
  (multi-source-mismatch fixture).
- `k8s/deployment.yaml` — Deployment that mounts the SealedSecret + ConfigMap.
- `Dockerfile` / `docker-compose.yml` / `.github/workflows/ci.yml` — additional
  declaration sources spanning the rest of the supported sensors.

E2E tests (`tests/e2e_cli.rs::env_truth`) copy this tree into a tempdir,
adjust mtimes to materialise drift, and exercise:

- `loct env-truth --json` — schema shape.
- `loct env-truth --name DATABASE_URL --json` — chain depth.
- `loct env-truth --fail-on stale-sealed-overrides-fresh-plain` — exit 2.
- `loct env-truth --fail-on multi-source-mismatch` — exit 2.

The encrypted payloads are intentionally **bogus base64** — even if a
loctree regression accidentally tried to decode them, no real secret would
leak. The discipline is "never decode" not "decode placeholders."
