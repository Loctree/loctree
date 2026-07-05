// Test-dir import: the only consumer of assetNameForPlatform.
import { assetNameForPlatform } from '../src/platform';

export function checkAsset(): boolean {
  return assetNameForPlatform('darwin-arm64').length > 0;
}
