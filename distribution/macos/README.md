# macOS

This channel is for direct Loctree downloads outside Homebrew and npm.

Target shape:

- signed `loct`, `loctree`, `loctree-mcp`, `loctree-lsp`, `aicx`, and
  `aicx-mcp` binaries inside the release tarball
- mandatory SHA256 sidecars for release artifacts
- optional GPG detached signatures (`.sig`) for release tarballs
- optional installer package later if we want a friendlier non-terminal path

Current 0.13.0 bundle direction:

- sign binaries with Developer ID Application
- package the signed binaries in the cross-platform `tar.gz` bundle
- write local artifacts to `dist/release-bundles/<version>/`
- publish the bundle through loct.io `public/releases/<version>/` when sync is enabled
- verify artifact integrity with SHA256 and, when configured, GPG detached signatures
- run `distribution/macos/smoke-releaseability.sh` before packaging so releases fail on non-system dylib paths such as `/opt/homebrew/...`

Notarized `.zip` delivery is not part of the 0.13.0 six-binary bundle flow.

Releaseability smoke path:

```bash
make smoke-release-macos-arm64 SMOKE_BIN_DIR=/path/to/staged/bin
```

Apple references:

- Signing Mac Software with Developer ID

## Maintainer notarization profile

Local notarization uses a stored `notarytool` keychain profile. Export the
Apple ID, team ID and app-specific password from your secret store first (never
commit them), then store the profile once per machine:

```bash
xcrun notarytool store-credentials "${NOTARY_PROFILE:-vc-notary}" \
  --apple-id "$NOTARY_APPLE_ID" \
  --team-id "$NOTARY_TEAM_ID" \
  --password "$NOTARY_PASSWORD"
```

CI does not use this profile: `distribution/macos/sign-and-notarize.sh` signs
in with the App Store Connect API key (`APPLE_API_KEY_BASE64`,
`APPLE_API_KEY_ID`, `APPLE_API_ISSUER_ID`).
