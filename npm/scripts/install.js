#!/usr/bin/env node

const https = require('https');
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { execFileSync } = require('child_process');

const VERSION = 'v0.3.0';
const REPO = 'sluisr/corex';

const PLATFORM_MAP = {
  linux: {
    x64: {
      archive: `corex-${VERSION}-x86_64-unknown-linux-gnu.tar.gz`,
      binName: 'cx',
      type: 'tar',
      sha256: '5a9117b823c5b438d8f8aa58683806b0d100ea39644bc5501bbb125c37d0d955'
    }
  },
  darwin: {
    arm64: {
      archive: `corex-${VERSION}-aarch64-apple-darwin.tar.gz`,
      binName: 'cx',
      type: 'tar',
      sha256: 'e71159e417f8179f7fdf5a5101c03945c19b8abc23698995fec5f13e5dedbd59'
    },
    x64: {
      // Fallback for Intel macs or Rosetta
      archive: `corex-${VERSION}-x86_64-apple-darwin.tar.gz`,
      binName: 'cx',
      type: 'tar',
      sha256: 'd82c0e3a8aa3318c88bc3389ed36c56dff932688b00c4af9d6a6bf16c7d44659'
    }
  },
  win32: {
    x64: {
      archive: `corex-${VERSION}-x86_64-pc-windows-msvc.zip`,
      binName: 'cx.exe',
      type: 'zip',
      sha256: '17233b981b568de646d8a6265d378e9b76e49fb4503f19e212aabb1100756090'
    }
  }
};

function sha256Of(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

// Verifies a file against the published checksum. On mismatch the file is removed and an error is
// thrown, so callers can fall back to a fresh download rather than trust a corrupted binary.
function verifyChecksum(file, expected) {
  if (!expected) {
    throw new Error('No SHA-256 checksum published for this platform; refusing to trust the binary.');
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

  const target = PLATFORM_MAP[platform]?.[arch];
  if (!target) {
    throw new Error(
      `Unsupported platform: ${platform} (${arch}). Please build from source via cargo: cargo install --git https://github.com/${REPO}`
    );
  }

  return target;
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
          fileStream.close();
          resolve();
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
  const binDir = path.join(__dirname, '..', 'bin');
  const targetBinPath = path.join(binDir, target.binName);

  if (!fs.existsSync(binDir)) {
    fs.mkdirSync(binDir, { recursive: true });
  }

  // If the binary already exists, re-verify its checksum before trusting it. Skipping
  // verification here would let a tampered or corrupted binary (planted by a preinstall script or
  // a previous failed download) execute unverified — exactly what the checksum guards against.
  if (fs.existsSync(targetBinPath)) {
    try {
      verifyChecksum(targetBinPath, target.sha256);
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
  console.log(`[corex] Verifying SHA-256 checksum (${target.sha256.slice(0, 16)}...)...`);
  verifyChecksum(archivePath, target.sha256);
  console.log('[corex] SHA-256 checksum verified successfully.');

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

module.exports = { install, getBinaryInfo, verifyChecksum };
