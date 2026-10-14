import assert from "node:assert/strict";
import test from "node:test";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import {
  assertPackageVersion,
  classifyNpmView,
  publishArgs,
} from "./publish-if-missing.mjs";

test("CI publishes with provenance by default", () => {
  assert.deepEqual(publishArgs(), ["publish", "--access", "public", "--provenance"]);
});

test("operator bootstrap can explicitly omit unavailable CI provenance", () => {
  assert.deepEqual(publishArgs(false), ["publish", "--access", "public"]);
});

test("requested immutable version must match package metadata", () => {
  assert.doesNotThrow(() => assertPackageVersion("0.14.4", "0.14.4"));
  assert.throws(
    () => assertPackageVersion("0.14.3", "0.14.4"),
    /does not match requested 0\.14\.4/,
  );
});

test("exact immutable version is skipped", () => {
  assert.equal(
    classifyNpmView({ status: 0, stdout: '"0.14.3"\n', stderr: "" }, "0.14.3"),
    "already-published",
  );
});

test("registry 404 is the only publishable missing state", () => {
  assert.equal(
    classifyNpmView(
      { status: 1, stdout: "", stderr: "npm error code E404\n404 Not Found" },
      "0.14.3",
    ),
    "missing",
  );
});

test("authentication and transport errors fail closed", () => {
  assert.throws(
    () =>
      classifyNpmView(
        { status: 1, stdout: "", stderr: "npm error code E401" },
        "0.14.3",
      ),
    /failed without a registry 404/,
  );
});

test("unexpected registry version fails closed", () => {
  assert.throws(
    () =>
      classifyNpmView({ status: 0, stdout: '"0.14.2"\n', stderr: "" }, "0.14.3"),
    /unexpected version/,
  );
});


test("OIDC release publication refuses local execution without falling back to a token", () => {
  const script = fileURLToPath(new URL("./publish-release.sh", import.meta.url));
  const result = spawnSync("bash", [script], {
    encoding: "utf8",
    env: {
      ...process.env,
      VERSION: "0.15.0",
      AUTH_MODE: "oidc",
      PUBLISH_CONFIRM: "1",
      GITHUB_ACTIONS: "false",
      ACTIONS_ID_TOKEN_REQUEST_URL: "",
      ACTIONS_ID_TOKEN_REQUEST_TOKEN: "",
      KEYS: "/nonexistent-bootstrap-credentials",
    },
  });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /requires the GitHub Actions trusted-publisher identity/);
  assert.doesNotMatch(result.stderr, /credential missing|whoami/);
});
