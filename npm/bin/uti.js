#!/usr/bin/env node

const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');
const { install, getBinaryInfo } = require('../scripts/install');

async function main() {
  const target = getBinaryInfo();
  const binDir = path.join(__dirname);
  const targetBinPath = path.join(binDir, target.binName);

  if (!fs.existsSync(targetBinPath)) {
    console.log('[uti-cli] Native binary not found, downloading...');
    await install();
  }

  const args = process.argv.slice(2);
  const result = spawnSync(targetBinPath, args, {
    stdio: 'inherit',
    env: process.env
  });

  if (result.error) {
    console.error(`[uti-cli] Failed to execute binary: ${result.error.message}`);
    process.exit(1);
  }

  process.exit(result.status ?? 0);
}

main().catch((err) => {
  console.error(`[uti-cli] Error: ${err.message}`);
  process.exit(1);
});
