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
const UPDATE_HANDOFF_SCHEMA_VERSION = 2;
const npmPackage = process.env.FASTCTX_NPM_PACKAGE || 'fastctx';
const npmLauncher = process.env.FASTCTX_NPM_LAUNCHER || __filename;
const npmMode = process.env.npm_command === 'exec' || npmLauncher
  .split(/[\\/]+/)
  .some((segment) => segment.toLowerCase() === '_npx')
  ? 'exec'
  : 'global';
const npmHandoff = path.join(
  fastctxHome,
  '.fastctx',
  'update',
  `npm-launcher-${process.pid}.handoff`,
);

function canonicalRegularFile(candidate) {
  if (!candidate || !path.isAbsolute(candidate)) return null;
  try {
    const canonical = fs.realpathSync(candidate);
    return fs.statSync(canonical).isFile() ? canonical : null;
  } catch (_) {
    return null;
  }
}

function isNpmCliScript(candidate) {
  return /^npm-cli\.(?:js|cjs|mjs)$/i.test(path.basename(candidate));
}

function nodeScriptInvocation(candidate) {
  const canonical = canonicalRegularFile(candidate);
  return canonical && isNpmCliScript(canonical)
    ? { driver: 'node-script', npmCli: canonical }
    : null;
}

function adjacentNpmCliCandidates(command) {
  const directory = path.dirname(command);
  return [
    path.join(directory, 'node_modules', 'npm', 'bin', 'npm-cli.js'),
    path.resolve(directory, '..', 'node_modules', 'npm', 'bin', 'npm-cli.js'),
    path.resolve(directory, '..', 'lib', 'node_modules', 'npm', 'bin', 'npm-cli.js'),
  ];
}

function commandInvocation(candidate) {
  const canonical = canonicalRegularFile(candidate);
  if (!canonical) return null;
  if (isNpmCliScript(canonical)) return { driver: 'node-script', npmCli: canonical };
  const invocationPath = path.resolve(candidate);

  const extension = path.extname(candidate).toLowerCase();
  if (process.platform === 'win32') {
    if (extension === '.cmd' || extension === '.bat') {
      for (const npmCli of adjacentNpmCliCandidates(candidate)) {
        const invocation = nodeScriptInvocation(npmCli);
        if (invocation) return invocation;
      }
      return null;
    }
    return extension === '.exe' || extension === '.com'
      ? { driver: 'executable', npmCli: invocationPath }
      : null;
  }

  try {
    fs.accessSync(invocationPath, fs.constants.X_OK);
    // 2026-07-22: Volta-style shims dispatch by argv[0], so realpath is validation-only here.
    return { driver: 'executable', npmCli: invocationPath };
  } catch (_) {
    return null;
  }
}

function launcherInstallCandidates() {
  const canonicalLauncher = canonicalRegularFile(npmLauncher);
  if (!canonicalLauncher) return { npmCli: [], commands: [] };
  const packageRoot = path.dirname(canonicalLauncher);
  const nodeModules = path.dirname(packageRoot);
  if (path.basename(nodeModules).toLowerCase() !== 'node_modules') {
    return { npmCli: [], commands: [] };
  }
  const modulesParent = path.dirname(nodeModules);
  const prefix = path.basename(modulesParent).toLowerCase() === 'lib'
    ? path.dirname(modulesParent)
    : modulesParent;
  return {
    npmCli: [path.join(nodeModules, 'npm', 'bin', 'npm-cli.js')],
    commands: process.platform === 'win32'
      ? [path.join(prefix, 'npm.exe'), path.join(prefix, 'npm.com'), path.join(prefix, 'npm.cmd'), path.join(prefix, 'npm.bat')]
      : [path.join(prefix, 'bin', 'npm')],
  };
}

function nodeLayoutCandidates() {
  const nodeExecutables = [process.execPath];
  const canonicalNode = canonicalRegularFile(process.execPath);
  if (canonicalNode && canonicalNode !== process.execPath) nodeExecutables.push(canonicalNode);
  return [...new Set(nodeExecutables.flatMap((nodeExecutable) => {
    const directory = path.dirname(nodeExecutable);
    return [
      path.join(directory, 'node_modules', 'npm', 'bin', 'npm-cli.js'),
      path.resolve(directory, '..', 'lib', 'node_modules', 'npm', 'bin', 'npm-cli.js'),
    ];
  }))];
}

function environmentValue(name) {
  const entry = Object.entries(process.env)
    .find(([key]) => key.toLowerCase() === name.toLowerCase());
  return entry && entry[1];
}

function pathCommandCandidates() {
  const pathValue = environmentValue('PATH');
  if (!pathValue) return [];
  const names = process.platform === 'win32'
    ? ['npm', ...((environmentValue('PATHEXT') || '.COM;.EXE;.BAT;.CMD')
      .split(';')
      .map((extension) => extension.trim().toLowerCase())
      .filter((extension, index, extensions) =>
        ['.com', '.exe', '.bat', '.cmd'].includes(extension) && extensions.indexOf(extension) === index)
      .map((extension) => `npm${extension}`))]
    : ['npm'];
  return pathValue.split(path.delimiter).flatMap((entry) => {
    const directory = entry.trim().replace(/^"(.*)"$/, '$1');
    // Skip empty and relative PATH entries: resolving them against the working
    // directory would let a planted `npm` drive an update (2026-07-22).
    if (!directory || !path.isAbsolute(directory)) return [];
    return names.map((name) => path.join(directory, name));
  });
}

function resolveNpmInvocation() {
  const explicit = commandInvocation(process.env.npm_execpath);
  if (explicit) return explicit;

  const launcherCandidates = launcherInstallCandidates();
  for (const candidate of launcherCandidates.npmCli) {
    const invocation = nodeScriptInvocation(candidate);
    if (invocation) return invocation;
  }
  for (const candidate of launcherCandidates.commands) {
    const invocation = commandInvocation(candidate);
    if (invocation) return invocation;
  }
  for (const candidate of nodeLayoutCandidates()) {
    const invocation = nodeScriptInvocation(candidate);
    if (invocation) return invocation;
  }
  for (const candidate of pathCommandCandidates()) {
    const invocation = commandInvocation(candidate);
    if (invocation) return invocation;
  }
  return { driver: 'unavailable', npmCli: '' };
}

const npmInvocation = resolveNpmInvocation();
const childEnvironment = { ...process.env };
for (const name of [
  'FASTCTX_NPM_LAUNCHER_VERSION',
  'FASTCTX_NPM_PACKAGE',
  'FASTCTX_NPM_MODE',
  'FASTCTX_NODE_EXECUTABLE',
  'FASTCTX_NPM_DRIVER',
  'FASTCTX_NPM_CLI',
  'FASTCTX_NPM_LAUNCHER',
  'FASTCTX_NPM_LAUNCHER_PID',
  'FASTCTX_NPM_HANDOFF',
]) {
  delete childEnvironment[name];
}
if (tuiLaunch) {
  const receiptVersion = npmInvocation.driver === 'node-script' ? '1' : '2';
  const receipt = {
    FASTCTX_NPM_LAUNCHER_VERSION: receiptVersion,
    FASTCTX_NPM_PACKAGE: npmPackage,
    FASTCTX_NPM_MODE: npmMode,
    FASTCTX_NODE_EXECUTABLE: process.execPath,
    FASTCTX_NPM_CLI: npmInvocation.npmCli,
    FASTCTX_NPM_LAUNCHER: npmLauncher,
    FASTCTX_NPM_LAUNCHER_PID: String(process.pid),
    FASTCTX_NPM_HANDOFF: npmHandoff,
  };
  if (receiptVersion === '2') receipt.FASTCTX_NPM_DRIVER = npmInvocation.driver;
  Object.assign(childEnvironment, receipt);
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

function readHandoff(expectedHelperExecutable, expectedHelperPid) {
  const payload = JSON.parse(fs.readFileSync(npmHandoff, 'utf8'));
  if (!payload || typeof payload !== 'object' || Array.isArray(payload)) {
    throw new Error('the update handoff is not an object');
  }
  if (payload.schema_version !== UPDATE_HANDOFF_SCHEMA_VERSION) {
    throw new Error(`unsupported update handoff schema ${payload.schema_version}`);
  }
  if (!['running', 'done', 'failed'].includes(payload.state)) {
    throw new Error(`unsupported update handoff state ${JSON.stringify(payload.state)}`);
  }
  if (!Number.isInteger(payload.helper_pid) || payload.helper_pid <= 0) {
    throw new Error('the update handoff has an invalid helper PID');
  }
  if (typeof payload.helper_executable !== 'string' || !path.isAbsolute(payload.helper_executable)) {
    throw new Error('the update handoff has an invalid helper path');
  }
  const updateDirectory = path.resolve(path.dirname(npmHandoff));
  const helperExecutable = path.resolve(payload.helper_executable);
  const relativeHelper = path.relative(updateDirectory, helperExecutable);
  if (
    !relativeHelper ||
    path.isAbsolute(relativeHelper) ||
    path.dirname(relativeHelper) !== '.' ||
    !path.basename(relativeHelper).startsWith('helper-')
  ) {
    throw new Error('the update handoff helper is outside the private update directory');
  }
  if (expectedHelperExecutable && helperExecutable !== expectedHelperExecutable) {
    throw new Error('the update handoff changed helper paths');
  }
  if (expectedHelperPid && payload.helper_pid !== expectedHelperPid) {
    throw new Error('the update handoff changed helper PIDs');
  }
  if (payload.detail !== undefined && typeof payload.detail !== 'string') {
    throw new Error('the update handoff has an invalid detail');
  }
  return { ...payload, helper_executable: helperExecutable };
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
  const expectedHelperExecutable = payload.helper_executable;
  const expectedHelperPid = payload.helper_pid;
  const poll = setInterval(() => {
    try {
      payload = readHandoff(expectedHelperExecutable, expectedHelperPid);
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
