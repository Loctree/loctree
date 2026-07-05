import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import assert from 'node:assert/strict';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const fixtureDir = join(__dirname, '../fixtures/dist-test');
const loctreeBin = join(__dirname, '../../target/release/loctree');

describe('dist command', () => {
  it('should identify unused exports using source maps', () => {
    // Run loctree dist command on the fixture
    const result = execFileSync(
      loctreeBin,
      ['dist', fixtureDir, '--format', 'json'],
      { encoding: 'utf8' }
    );

    const output = JSON.parse(result);

    // Should find unused exports
    const unusedExports = output.unusedExports || [];
    const unusedNames = unusedExports.map(e => e.name);

    // These should be detected as unused (not in source map names)
    assert.ok(unusedNames.includes('unusedFunction'), 'should detect unusedFunction');
    assert.ok(unusedNames.includes('UNUSED_CONST'), 'should detect UNUSED_CONST');
    assert.ok(unusedNames.includes('UnusedClass'), 'should detect UnusedClass');
    assert.ok(unusedNames.includes('deadHelper'), 'should detect deadHelper');

    // These should NOT be in unused list (they are in source map names)
    assert.ok(!unusedNames.includes('usedFunction'), 'usedFunction should not be unused');
    assert.ok(!unusedNames.includes('USED_CONST'), 'USED_CONST should not be unused');
    assert.ok(!unusedNames.includes('UsedClass'), 'UsedClass should not be unused');
    assert.ok(!unusedNames.includes('helper'), 'helper should not be unused');
  });

  it('should find source map files recursively', () => {
    const result = execFileSync(
      loctreeBin,
      ['dist', fixtureDir, '--format', 'json'],
      { encoding: 'utf8' }
    );

    const output = JSON.parse(result);

    // Should find the source map file
    assert.ok(output.sourceMapsFound > 0, 'should find at least one source map');
  });
});
