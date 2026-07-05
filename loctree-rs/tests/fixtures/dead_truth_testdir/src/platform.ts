// Mirrors the assetNameForPlatform empiria: the only consumer lives in a
// test directory, which the import graph historically missed.
export function assetNameForPlatform(platform: string): string {
  return `loctree-lsp-${platform}.vsix`;
}

// A genuinely dead export: no importer, no literal hit anywhere else.
export function trulyDeadHelper(): number {
  return 42;
}
