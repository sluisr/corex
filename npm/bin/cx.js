#!/usr/bin/env node

const fs = require('fs');
const { spawnSync } = require('child_process');
const { install, getBinaryPath } = require('../scripts/install');

async function main() {
  let targetBinPath = getBinaryPath();

  if (!fs.existsSync(targetBinPath)) {
    console.log('[corex] First run: downloading native binary from GitHub...');
    await install();
    targetBinPath = getBinaryPath();
  }

  const args = process.argv.slice(2);
  const result = spawnSync(targetBinPath, args, {
    stdio: 'inherit',
    env: process.env
  });

  if (result.error) {
    console.error(`[corex] Failed to execute binary: ${result.error.message}`);
    process.exit(1);
  }

  process.exit(result.status ?? 0);
}

main().catch((err) => {
  console.error(`[corex] Error: ${err.message}`);
  process.exit(1);
});
