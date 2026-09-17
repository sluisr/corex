#!/usr/bin/env node

const https = require('https');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const VERSION = 'v0.2.0';
const REPO = 'sluisr/uti-cli';

const PLATFORM_MAP = {
  linux: {
    x64: {
      archive: `uti-${VERSION}-x86_64-unknown-linux-gnu.tar.gz`,
      binName: 'uti',
      type: 'tar'
    }
  },
  darwin: {
    arm64: {
      archive: `uti-${VERSION}-aarch64-apple-darwin.tar.gz`,
      binName: 'uti',
      type: 'tar'
    },
    x64: {
      // Fallback for Intel macs or Rosetta
      archive: `uti-${VERSION}-aarch64-apple-darwin.tar.gz`,
      binName: 'uti',
      type: 'tar'
    }
  },
  win32: {
    x64: {
      archive: `uti-${VERSION}-x86_64-pc-windows-msvc.zip`,
      binName: 'uti.exe',
      type: 'zip'
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

  console.log(`[uti-cli] UTI CLI v0.2.0 successfully installed to ${targetBinPath}`);
}

if (require.main === module) {
  install().catch((err) => {
    console.error(`[uti-cli] Installation error: ${err.message}`);
    process.exit(1);
  });
}

module.exports = { install, getBinaryInfo };
