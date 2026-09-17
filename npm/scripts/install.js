#!/usr/bin/env node

const https = require('https');
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { execSync } = require('child_process');

const VERSION = 'v0.2.0';
const REPO = 'sluisr/uti-cli';

const PLATFORM_MAP = {
  linux: {
    x64: {
      archive: `uti-${VERSION}-x86_64-unknown-linux-gnu.tar.gz`,
      binName: 'uti',
      type: 'tar',
      sha256: '7b67e49b4bbc44eb5ac8f41fad69f9610b6c4486c77b8963c4affaadd307aac6'
    }
  },
  darwin: {
    arm64: {
      archive: `uti-${VERSION}-aarch64-apple-darwin.tar.gz`,
      binName: 'uti',
      type: 'tar',
      sha256: '1d97f877286447245d25ebda2c06fcc935ecd5eddd5396df2539212b1fb6af57'
    },
    x64: {
      // Fallback for Intel macs or Rosetta
      archive: `uti-${VERSION}-aarch64-apple-darwin.tar.gz`,
      binName: 'uti',
      type: 'tar',
      sha256: '1d97f877286447245d25ebda2c06fcc935ecd5eddd5396df2539212b1fb6af57'
    }
  },
  win32: {
    x64: {
      archive: `uti-${VERSION}-x86_64-pc-windows-msvc.zip`,
      binName: 'uti.exe',
      type: 'zip',
      sha256: 'e2bde564c8e59745b279fd9a9b060ae50ab2a1c5872d15666b323dfd18cf148a'
    }
  }
};

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

function downloadFile(url, dest) {
  return new Promise((resolve, reject) => {
    https
      .get(url, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          // Follow redirect
          return downloadFile(res.headers.location, dest).then(resolve).catch(reject);
        }

        if (res.statusCode !== 200) {
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
      })
      .on('error', reject);
  });
}

async function install() {
  const target = getBinaryInfo();
  const binDir = path.join(__dirname, '..', 'bin');
  const targetBinPath = path.join(binDir, target.binName);

  if (!fs.existsSync(binDir)) {
    fs.mkdirSync(binDir, { recursive: true });
  }

  // If binary already exists and is executable, skip
  if (fs.existsSync(targetBinPath)) {
    try {
      if (process.platform !== 'win32') {
        fs.chmodSync(targetBinPath, 0o755);
      }
      return;
    } catch (_) {}
  }

  const url = `https://github.com/${REPO}/releases/download/${VERSION}/${target.archive}`;
  const archivePath = path.join(binDir, target.archive);

  console.log(`[uti-cli] Downloading native binary for ${process.platform}-${process.arch} from GitHub...`);
  await downloadFile(url, archivePath);

  // Corporate-grade cryptographic integrity verification
  if (target.sha256) {
    console.log(`[uti-cli] Verifying SHA-256 checksum (${target.sha256.slice(0, 16)}...)...`);
    const fileBuffer = fs.readFileSync(archivePath);
    const calculatedHash = crypto.createHash('sha256').update(fileBuffer).digest('hex');
    if (calculatedHash.toLowerCase() !== target.sha256.toLowerCase()) {
      try { fs.unlinkSync(archivePath); } catch (_) {}
      throw new Error(
        `SHA-256 verification failed! Potential corrupted download or security tampering.\nExpected: ${target.sha256}\nReceived: ${calculatedHash}`
      );
    }
    console.log('[uti-cli] SHA-256 checksum verified successfully.');
  }

  console.log('[uti-cli] Extracting binary...');
  try {
    if (target.type === 'tar') {
      execSync(`tar -xzf "${archivePath}" -C "${binDir}"`, { stdio: 'inherit' });
    } else if (target.type === 'zip') {
      if (process.platform === 'win32') {
        execSync(`powershell -command "Expand-Archive -Force -Path '${archivePath}' -DestinationPath '${binDir}'"`, {
          stdio: 'inherit'
        });
      } else {
        execSync(`unzip -o "${archivePath}" -d "${binDir}"`, { stdio: 'inherit' });
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

  console.log(`[uti-cli] UTI CLI ${VERSION} successfully verified and installed to ${targetBinPath}`);
}

if (require.main === module) {
  install().catch((err) => {
    console.error(`[uti-cli] Installation error: ${err.message}`);
    process.exit(1);
  });
}

module.exports = { install, getBinaryInfo };
