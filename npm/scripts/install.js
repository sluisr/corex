#!/usr/bin/env node

const https = require('https');
const fs = require('fs');
const path = require('path');
const os = require('os');
const crypto = require('crypto');
const { execFileSync } = require('child_process');

const VERSION = 'v0.5.0';
const REPO = 'sluisr/corex';

const PLATFORM_MAP = {
  linux: {
    x64: {
      archive: `corex-${VERSION}-x86_64-unknown-linux-gnu.tar.gz`,
      binName: 'cx',
      type: 'tar',
      sha256: 'auto'
    }
  },
  darwin: {
    arm64: {
      archive: `corex-${VERSION}-aarch64-apple-darwin.tar.gz`,
      binName: 'cx',
      type: 'tar',
      sha256: 'auto'
    },
    x64: {
      // Fallback for Intel macs or Rosetta
      archive: `corex-${VERSION}-x86_64-apple-darwin.tar.gz`,
      binName: 'cx',
      type: 'tar',
      sha256: 'auto'
    }
  },
  win32: {
    x64: {
      archive: `corex-${VERSION}-x86_64-pc-windows-msvc.zip`,
      binName: 'cx.exe',
      type: 'zip',
      sha256: 'auto'
    }
  }
};

function sha256Of(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

// Verifies a file against the published checksum. On mismatch the file is removed and an error is
// thrown, so callers can fall back to a fresh download rather than trust a corrupted binary.
function verifyChecksum(file, expected) {
  if (!expected || expected === 'auto') {
    return;
  }
  const actual = sha256Of(file);
  if (actual.toLowerCase() !== expected.toLowerCase()) {
    try { fs.unlinkSync(file); } catch (_) {}
    throw new Error(
      `SHA-256 verification failed! Potential corrupted download or security tampering.\nExpected: ${expected}\nReceived: ${actual}`
    );
  }
}

function getBinaryInfo() {
  const platform = process.platform;
  const arch = process.arch;

  const target = PLATFORM_MAP[platform] && PLATFORM_MAP[platform][arch];
  if (!target) {
    throw new Error(
      `Unsupported operating system / architecture: ${platform}-${arch}. Supported: Linux (x64), macOS (arm64, x64), Windows (x64).`
    );
  }
  return target;
}

function getBinDir() {
  // Check if local package bin directory is writable
  const localBin = path.join(__dirname, '..', 'bin');
  try {
    fs.mkdirSync(localBin, { recursive: true });
    fs.accessSync(localBin, fs.constants.W_OK);
    return localBin;
  } catch (_) {
    // Fall back to ~/.corex/bin which is always writable by the current user without sudo
    const userBin = path.join(os.homedir(), '.corex', 'bin');
    fs.mkdirSync(userBin, { recursive: true });
    return userBin;
  }
}

function getBinaryPath() {
  const target = getBinaryInfo();
  // 1. Check local package bin first
  const localBin = path.join(__dirname, '..', 'bin', target.binName);
  if (fs.existsSync(localBin)) {
    return localBin;
  }
  // 2. Check user cache directory (~/.corex/bin)
  const userBin = path.join(os.homedir(), '.corex', 'bin', target.binName);
  if (fs.existsSync(userBin)) {
    return userBin;
  }
  // 3. Fallback to preferred target path
  return path.join(getBinDir(), target.binName);
}

function downloadFile(url, dest, redirectCount = 0) {
  const MAX_REDIRECTS = 5;
  const TIMEOUT_MS = 30000;

  return new Promise((resolve, reject) => {
    if (redirectCount > MAX_REDIRECTS) {
      return reject(new Error(`Too many redirects (limit ${MAX_REDIRECTS}): ${url}`));
    }

    let parsedUrl;
    try {
      parsedUrl = new URL(url);
    } catch (e) {
      return reject(new Error(`Invalid URL: ${url}`));
    }

    if (parsedUrl.protocol !== 'https:') {
      return reject(new Error(`Insecure download protocol rejected: ${parsedUrl.protocol}`));
    }

    const req = https.get(
      url,
      {
        timeout: TIMEOUT_MS,
        headers: { 'User-Agent': 'corex-npm-installer' }
      },
      (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          const nextUrl = new URL(res.headers.location, url).toString();
          res.resume(); // Discard redirect body
          return downloadFile(nextUrl, dest, redirectCount + 1).then(resolve).catch(reject);
        }

        if (res.statusCode !== 200) {
          res.resume();
          return reject(new Error(`Download failed with status HTTP ${res.statusCode}: ${url}`));
        }

        const fileStream = fs.createWriteStream(dest);
        res.pipe(fileStream);

        fileStream.on('finish', () => {
          fileStream.close(resolve);
        });

        fileStream.on('error', (err) => {
          fs.unlink(dest, () => reject(err));
        });
      }
    );

    req.on('timeout', () => {
      req.destroy(new Error(`Download timed out after ${TIMEOUT_MS / 1000}s: ${url}`));
    });

    req.on('error', (err) => {
      if (fs.existsSync(dest)) {
        try { fs.unlinkSync(dest); } catch (_) {}
      }
      reject(err);
    });
  });
}

async function install() {
  const target = getBinaryInfo();
  const binDir = getBinDir();
  const targetBinPath = path.join(binDir, target.binName);

  if (fs.existsSync(targetBinPath)) {
    try {
      if (target.sha256 && target.sha256 !== 'auto') {
        verifyChecksum(targetBinPath, target.sha256);
      }
      if (process.platform !== 'win32') {
        fs.chmodSync(targetBinPath, 0o755);
      }
      return;
    } catch (err) {
      console.log(`[corex] Existing binary rejected (${err.message.split('\n')[0]}); re-downloading...`);
    }
  }

  const url = `https://github.com/${REPO}/releases/download/${VERSION}/${target.archive}`;
  const archivePath = path.join(binDir, target.archive);

  console.log(`[corex] Downloading native binary for ${process.platform}-${process.arch} from GitHub...`);
  await downloadFile(url, archivePath);

  // Cryptographic integrity verification of the freshly downloaded archive.
  // 1. Automatically fetch the published .sha256 from GitHub Release if available
  // 2. Or fallback to hardcoded target.sha256
  let expectedSha = target.sha256;
  try {
    const shaUrl = `${url}.sha256`;
    const shaPath = path.join(binDir, `${target.archive}.sha256`);
    await downloadFile(shaUrl, shaPath);
    const publishedSha = fs.readFileSync(shaPath, 'utf8').trim().split(/\s+/)[0];
    if (publishedSha && /^[a-fA-F0-9]{64}$/.test(publishedSha)) {
      expectedSha = publishedSha;
    }
    try { fs.unlinkSync(shaPath); } catch (_) {}
  } catch (_) {
    // If .sha256 file is not yet available, fallback gracefully
  }

  if (expectedSha && expectedSha !== 'auto') {
    console.log(`[corex] Verifying SHA-256 checksum (${expectedSha.slice(0, 16)}...)...`);
    verifyChecksum(archivePath, expectedSha);
    console.log('[corex] SHA-256 checksum verified successfully.');
  }

  console.log('[corex] Extracting binary...');
  try {
    if (target.type === 'tar') {
      execFileSync('tar', ['-xzf', archivePath, '-C', binDir], { stdio: 'inherit' });
    } else if (target.type === 'zip') {
      if (process.platform === 'win32') {
        execFileSync(
          'powershell.exe',
          [
            '-NoProfile',
            '-NonInteractive',
            '-Command',
            'Expand-Archive -Force -LiteralPath $args[0] -DestinationPath $args[1]',
            archivePath,
            binDir
          ],
          { stdio: 'inherit' }
        );
      } else {
        execFileSync('unzip', ['-o', archivePath, '-d', binDir], { stdio: 'inherit' });
      }
    }
  } finally {
    if (fs.existsSync(archivePath)) {
      try {
        fs.unlinkSync(archivePath);
      } catch (_) {}
    }
  }

  if (process.platform !== 'win32' && fs.existsSync(targetBinPath)) {
    fs.chmodSync(targetBinPath, 0o755);
  }

  console.log(`[corex] Corex ${VERSION} successfully verified and installed to ${targetBinPath}`);
}

if (require.main === module) {
  install().catch((err) => {
    console.error(`[corex] Installation error: ${err.message}`);
    process.exit(1);
  });
}

module.exports = { install, getBinaryInfo, getBinaryPath, verifyChecksum };
