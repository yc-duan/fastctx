#!/usr/bin/env node
'use strict';

const { spawn } = require('node:child_process');
const { constants } = require('node:os');

const platforms = {
  'win32-x64': '@fastctx/win32-x64',
  'win32-arm64': '@fastctx/win32-arm64',
  'linux-x64': '@fastctx/linux-x64',
  'linux-arm64': '@fastctx/linux-arm64',
  'darwin-x64': '@fastctx/darwin-x64',
  'darwin-arm64': '@fastctx/darwin-arm64',
};

function main() {
  const platform = `${process.platform}-${process.arch}`;
  const name = platforms[platform];
  if (!name) throw new Error(`Unsupported platform: ${platform}`);
  const version = require('./package.json').version;
  const installed = require(`${name}/package.json`).version;
  if (installed !== version) {
    throw new Error(`${name} is ${installed}; this launcher requires ${version}`);
  }
  const binary = require.resolve(`${name}/bin/fastctx${process.platform === 'win32' ? '.exe' : ''}`);
  // Old upgraders start this exact file with their original Node executable.
  // Inherited handles keep the TUI's terminal; FINALIZE reaches native main.
  const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit', env: process.env });
  const signals = ['SIGINT', 'SIGTERM', 'SIGHUP'];
  const handlers = new Map(signals.map(signal => [signal, () => child.kill(signal)]));
  for (const [signal, handler] of handlers) process.on(signal, handler);
  child.once('error', error => {
    console.error(`fastctx: ${error.message}`);
    process.exitCode = 1;
  });
  child.once('exit', (code, signal) => {
    for (const [name, handler] of handlers) process.removeListener(name, handler);
    if (signal) {
      // Re-raise the signal so a Unix caller observes the child's termination.
      process.exitCode = 128 + constants.signals[signal];
      process.kill(process.pid, signal);
    } else {
      process.exitCode = code;
    }
  });
}

try {
  main();
} catch (error) {
  console.error(`fastctx: ${error.message}`);
  process.exitCode = 1;
}
