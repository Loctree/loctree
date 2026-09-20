#!/usr/bin/env node

const { execFileSync } = require('child_process');
const { existsSync } = require('fs');
const { basename, dirname, isAbsolute, join, normalize, relative, sep } = require('path');
const { fileURLToPath, pathToFileURL } = require('url');

// Loctree is one runtime. The npm wrapper exposes `loctree` (with `loct` as a
// short alias) and a narrow `loctree-mcp` adapter for stdio package runners.
// MCP and LSP remain co-process binaries the runtime can spawn on demand:
//
//   loct watch --http   -> loctree co-spawns `loctree-mcp` (streamable HTTP MCP)
//   loct watch --lsp    -> loctree co-spawns `loctree-lsp`  (editor language server)
//
// The runtime resolves those co-processes as SIBLINGS of its own executable,
// so the platform package must ship the suite binaries side by side. They all
// live under bin/ inside the platform package (`loct` is the same runtime as
// `loctree` under its short name).
const RUNTIME_BINARY = 'loctree';
const BUNDLED_BINARIES = ['loct', 'loctree', 'loctree-mcp', 'loctree-lsp'];

// Platform key -> the technical platform package that delivers the binaries.
// These are a delivery mechanism only; users never install them directly.
const PLATFORM_PACKAGES = {
  'darwin-arm64': '@loctree/loctree-darwin-arm64',
  'darwin-x64': '@loctree/loctree-darwin-x64',
  'linux-x64-gnu': '@loctree/loctree-linux-x64-gnu',
  'win32-x64-msvc': '@loctree/loctree-win32-x64-msvc',
};

/**
 * Normalize process.platform/process.arch into the key that selects a platform
 * package, adding the libc flavour on Linux. Null when Loctree ships no build.
 */
function getPlatformKey() {
  const platform = process.platform;
  const arch = process.arch;

  const archMap = {
    'x64': 'x64',
    'arm64': 'arm64',
    'aarch64': 'arm64',
  };
  const normalizedArch = archMap[arch] || arch;

  if (platform === 'linux') {
    const libc = isMuslLibc() ? 'musl' : 'gnu';
    return `${platform}-${normalizedArch}-${libc}`;
  }
  if (platform === 'win32') {
    return `${platform}-${normalizedArch}-msvc`;
  }
  if (platform === 'darwin') {
    return `${platform}-${normalizedArch}`;
  }
  return null;
}

/**
 * Detect a musl-based Linux from `ldd --version` output. Any failure is read as
 * glibc, so a missing ldd never breaks resolution on the common distro.
 */
function isMuslLibc() {
  const { spawnSync } = require('child_process');
  try {
    const lddVersion = spawnSync('ldd', ['--version'], { encoding: 'utf8' });
    return lddVersion.stderr && lddVersion.stderr.includes('musl');
  } catch (err) {
    return false;
  }
}

/** Map the current platform key to its @loctree/loctree-* package, or null. */
function platformPackageName() {
  const key = getPlatformKey();
  if (!key) return null;
  return PLATFORM_PACKAGES[key] || null;
}

/** Add the .exe suffix Windows binaries carry inside the platform package. */
function binaryFileName(name, platform = process.platform) {
  return platform === 'win32' ? `${name}.exe` : name;
}

/**
 * Join one binary file name onto the platform package's bin/ directory and refuse
 * anything that is not a plain name resolving inside it, so a crafted name cannot
 * make the wrapper execute a binary from outside the package.
 */
function childPathInsidePackage(pkgDir, fileName) {
  if (!/^[A-Za-z0-9._-]+$/.test(fileName) || basename(fileName) !== fileName) {
    throw new Error(`Unsafe Loctree binary file name: ${fileName}`);
  }

  const root = normalize(pkgDir);
  const rootUrl = pathToFileURL(root.endsWith(sep) ? root : `${root}${sep}`);
  const childPath = normalize(fileURLToPath(new URL(fileName, rootUrl)));
  const relativePath = relative(root, childPath);
  if (relativePath === '' || relativePath.startsWith('..') || isAbsolute(relativePath)) {
    throw new Error(`Loctree binary escaped platform package directory: ${fileName}`);
  }
  return childPath;
}

/**
 * Resolve the absolute path to one bundled binary inside the installed platform
 * package. Uses node module resolution (so it respects npm/pnpm/yarn hoisting),
 * then validates the file exists. The bundled binaries always live together in
 * the platform package's bin/ directory, which is what lets `loctree` find
 * `loctree-mcp` / `loctree-lsp` as siblings at runtime.
 */
function getBinaryPath(name = RUNTIME_BINARY) {
  if (!BUNDLED_BINARIES.includes(name)) {
    throw new Error(`Unknown Loctree binary: ${name}`);
  }

  const packageName = platformPackageName();
  if (!packageName) {
    throw new Error(
      `Unsupported platform: ${process.platform}-${process.arch}. ` +
      `Loctree ships for: ${Object.keys(PLATFORM_PACKAGES).join(', ')}.`
    );
  }

  let pkgDir;
  try {
    // Resolve via the platform package's manifest; the binaries sit in bin/.
    pkgDir = dirname(require.resolve(`${packageName}/package.json`));
  } catch (err) {
    throw new Error(
      `Loctree platform package "${packageName}" is not installed. ` +
      `This usually means optionalDependencies were disabled ` +
      `(--no-optional / --ignore-optional). Reinstall @loctree/loctree with optional deps enabled.`
    );
  }

  const binaryPath = childPathInsidePackage(join(pkgDir, 'bin'), binaryFileName(name));
  if (!existsSync(binaryPath)) {
    throw new Error(
      `Loctree binary "${binaryFileName(name)}" not found in "${packageName}" ` +
      `(expected at ${binaryPath}). The platform package may be incomplete.`
    );
  }
  return binaryPath;
}

/**
 * Run the Loctree runtime (`loctree`), inheriting stdio. Only the `loctree`
 * bin entry calls this — `loct` routes to its own banner-free binary via
 * `runBinary('loct', ...)`, so the deprecation nag never fires on the
 * preferred command name.
 * Exits with the binary's status code on failure.
 */
function runRuntime(args = []) {
  return runBinary(RUNTIME_BINARY, args);
}

/**
 * Run one bundled Loctree binary, inheriting stdio. This is intentionally
 * narrow: package entrypoints may expose a bundled co-process without adding a
 * second npm package or searching PATH for an unrelated install.
 */
function runBinary(name, args = []) {
  const binaryPath = getBinaryPath(name);
  try {
    return execFileSync(binaryPath, args, { stdio: 'inherit' });
  } catch (err) {
    if (typeof err.status === 'number') {
      process.exit(err.status);
    }
    throw err;
  }
}

// Backwards-compatible alias kept for any pre-existing importers of this module.
function execLoct(args = [], options = {}) {
  const binaryPath = getBinaryPath(RUNTIME_BINARY);
  try {
    return execFileSync(binaryPath, args, { stdio: 'inherit', ...options });
  } catch (err) {
    if (typeof err.status === 'number') {
      process.exit(err.status);
    }
    throw err;
  }
}

module.exports = {
  RUNTIME_BINARY,
  BUNDLED_BINARIES,
  platformPackageName,
  getPlatformKey,
  binaryFileName,
  getBinaryPath,
  runBinary,
  runRuntime,
  execLoct,
};

if (require.main === module) {
  runRuntime(process.argv.slice(2));
}
