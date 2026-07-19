#!/usr/bin/env node
'use strict';

const { spawn } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const fastctxHome = process.platform === 'win32'
  ? process.env.USERPROFILE || os.homedir()
  : process.env.HOME || os.homedir();

const targets = {
  'win32-x64': ['@fastctx/win32-x64', 'fastctx.exe'],
  'linux-x64': ['@fastctx/linux-x64', 'fastctx'],
  'darwin-x64': ['@fastctx/darwin-x64', 'fastctx'],
  'darwin-arm64': ['@fastctx/darwin-arm64', 'fastctx'],
};

const target = targets[`${process.platform}-${process.arch}`];
if (!target) {
  console.error(`fastctx: unsupported platform ${process.platform}-${process.arch}`);
  process.exit(1);
}

let executable;
let platformPackageMissing = false;
try {
  const packageRoot = path.dirname(require.resolve(`${target[0]}/package.json`));
  const packagedExecutable = path.join(packageRoot, 'bin', target[1]);
  if (!fs.statSync(packagedExecutable).isFile()) {
    platformPackageMissing = true;
  } else {
    executable = packagedExecutable;
  }
} catch (_) {
  platformPackageMissing = true;
}
if (platformPackageMissing) {
  const stableExecutable = path.join(fastctxHome, '.fastctx', 'bin', target[1]);
  let stableCopyReady = false;
  try {
    stableCopyReady = fs.statSync(stableExecutable).isFile();
  } catch (_) {
    stableCopyReady = false;
  }
  if (stableCopyReady) {
    executable = stableExecutable;
    console.error(
      `fastctx: platform package ${target[0]} is missing; using the stable copy at ${stableExecutable}`,
    );
  } else {
    console.error(
      [
        `fastctx: platform package ${target[0]} is missing, and no stable copy is installed.`,
        'Your configured npm registry may not have synchronized the platform package yet.',
        'Retry once from the official registry:',
        '  npm install --global fastctx --registry=https://registry.npmjs.org/',
      ].join('\n'),
    );
    process.exit(1);
  }
}
const args = process.argv.slice(2);
const interactive = Boolean(process.stdin.isTTY && process.stdout.isTTY && args[0] !== 'serve');
const tuiLaunch = interactive && (args.length === 0 || args[0] === 'ui');
const FORCE_KILL_DELAY_MS = 5000;
const UPDATE_HANDOFF_EXIT_CODE = 75;
const npmPackage = process.env.FASTCTX_NPM_PACKAGE || 'fastctx';
const npmLauncher = process.env.FASTCTX_NPM_LAUNCHER || __filename;
const npmMode = process.env.npm_command === 'exec' || npmLauncher.includes(`${path.sep}_npx${path.sep}`)
  ? 'exec'
  : 'global';
const npmHandoff = path.join(
  fastctxHome,
  '.fastctx',
  'update',
  `npm-launcher-${process.pid}.handoff`,
);
const npmCliCandidates = [
  process.env.npm_execpath,
  path.join(path.dirname(process.execPath), 'node_modules', 'npm', 'bin', 'npm-cli.js'),
  path.resolve(path.dirname(process.execPath), '..', 'lib', 'node_modules', 'npm', 'bin', 'npm-cli.js'),
].filter(Boolean);
const npmCli = npmCliCandidates.find((candidate) => path.isAbsolute(candidate) && fs.existsSync(candidate)) || '';
const childEnvironment = { ...process.env };
for (const name of [
  'FASTCTX_NPM_LAUNCHER_VERSION',
  'FASTCTX_NPM_PACKAGE',
  'FASTCTX_NPM_MODE',
  'FASTCTX_NODE_EXECUTABLE',
  'FASTCTX_NPM_CLI',
  'FASTCTX_NPM_LAUNCHER',
  'FASTCTX_NPM_LAUNCHER_PID',
  'FASTCTX_NPM_HANDOFF',
]) {
  delete childEnvironment[name];
}
if (tuiLaunch) {
  Object.assign(childEnvironment, {
    FASTCTX_NPM_LAUNCHER_VERSION: '1',
    FASTCTX_NPM_PACKAGE: npmPackage,
    FASTCTX_NPM_MODE: npmMode,
    FASTCTX_NODE_EXECUTABLE: process.execPath,
    FASTCTX_NPM_CLI: npmCli,
    FASTCTX_NPM_LAUNCHER: npmLauncher,
    FASTCTX_NPM_LAUNCHER_PID: String(process.pid),
    FASTCTX_NPM_HANDOFF: npmHandoff,
  });
}
const child = spawn(executable, args, {
  stdio: interactive ? 'inherit' : ['pipe', 'pipe', 'pipe'],
  windowsHide: true,
  env: childEnvironment,
});
let forwardedSignal = null;
let forceKillTimer = null;

function finish(code, signal) {
  if (forceKillTimer) clearTimeout(forceKillTimer);
  if (tuiLaunch && code === UPDATE_HANDOFF_EXIT_CODE && !signal) {
    waitForUpdateHandoff();
    return;
  }
  if (forwardedSignal && process.platform !== 'win32') {
    const forwarded = forwardedSignal;
    process.removeAllListeners(forwarded);
    process.kill(process.pid, forwarded);
    return;
  }
  process.exit(Number.isInteger(code) ? code : signal ? 1 : 0);
}

function processIsAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error && error.code === 'EPERM';
  }
}

function readHandoff() {
  return JSON.parse(fs.readFileSync(npmHandoff, 'utf8'));
}

function finishUpdateHandoff(payload, succeeded, detail) {
  let attempts = 0;
  const finishCleanup = () => {
    attempts += 1;
    let helperRemoved = false;
    try {
      fs.unlinkSync(payload.helper_executable);
      helperRemoved = true;
    } catch (error) {
      helperRemoved = error && error.code === 'ENOENT';
      if (!helperRemoved && attempts < 50) return;
    }
    try {
      fs.unlinkSync(npmHandoff);
    } catch (error) {
      if (!error || error.code !== 'ENOENT') {
        console.error(`fastctx: cannot remove update handoff: ${error.message}`);
      }
    }
    if (!helperRemoved) {
      console.error(`fastctx: updater helper cleanup was deferred: ${payload.helper_executable}`);
    }
    if (!succeeded) {
      console.error(`fastctx: update handoff failed${detail ? `: ${detail}` : ''}`);
    }
    process.exit(succeeded ? 0 : 1);
  };
  setInterval(finishCleanup, 100);
  finishCleanup();
}

function waitForUpdateHandoff() {
  let payload;
  try {
    payload = readHandoff();
  } catch (error) {
    console.error(`fastctx: cannot read update handoff: ${error.message}`);
    process.exit(1);
  }
  const poll = setInterval(() => {
    try {
      payload = readHandoff();
    } catch (error) {
      clearInterval(poll);
      console.error(`fastctx: cannot read update handoff: ${error.message}`);
      process.exit(1);
    }
    if (payload.state === 'done' || payload.state === 'failed') {
      clearInterval(poll);
      finishUpdateHandoff(payload, payload.state === 'done', payload.detail);
      return;
    }
    if (!Number.isInteger(payload.helper_pid) || !processIsAlive(payload.helper_pid)) {
      clearInterval(poll);
      finishUpdateHandoff(payload, false, 'the updater helper exited unexpectedly');
    }
  }, 100);
}

function forward(signal) {
  if (forwardedSignal || child.exitCode !== null || child.signalCode !== null) return;
  forwardedSignal = signal;
  try {
    child.kill(signal);
  } catch (_) {
    child.kill();
  }
  forceKillTimer = setTimeout(() => {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
  }, FORCE_KILL_DELAY_MS);
  forceKillTimer.unref();
}

for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP', 'SIGBREAK']) {
  try {
    process.on(signal, () => forward(signal));
  } catch (_) {
    // Some signals are unavailable on some platforms.
  }
}

child.once('error', (error) => {
  console.error(`fastctx: failed to start native binary: ${error.message}`);
  process.exit(1);
});
child.once('exit', finish);

if (!interactive) {
  process.stdin.pipe(child.stdin);
  child.stdout.pipe(process.stdout);
  child.stderr.pipe(process.stderr);
  child.stdin.on('error', (error) => {
    if (!['EPIPE', 'ERR_STREAM_PREMATURE_CLOSE', 'ERR_STREAM_WRITE_AFTER_END'].includes(error.code)) {
      console.error(`fastctx: stdin proxy failed: ${error.message}`);
    }
  });
}
