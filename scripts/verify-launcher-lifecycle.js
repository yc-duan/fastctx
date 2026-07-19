#!/usr/bin/env node
'use strict';

const { execFileSync, spawn, spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const readline = require('node:readline');

const launcher = process.argv[2];
if (!launcher) throw new Error('usage: verify-launcher-lifecycle.js <launcher.js>');
const SIGNAL_FORWARD_DEADLINE_MS = 3000;

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function childPids(parentPid) {
  try {
    if (process.platform === 'win32') {
      const script = `@(Get-CimInstance Win32_Process -Filter 'ParentProcessId = ${parentPid}' | Where-Object { $_.Name -ieq 'fastctx.exe' }).ProcessId`;
      const output = execFileSync('powershell.exe', ['-NoProfile', '-Command', script], {
        encoding: 'utf8',
        windowsHide: true,
      });
      return output
        .trim()
        .split(/\s+/)
        .filter(Boolean)
        .map(Number)
        .filter(Number.isInteger);
    }
    const output = execFileSync('pgrep', ['-P', String(parentPid)], { encoding: 'utf8' });
    return output
      .trim()
      .split(/\s+/)
      .filter(Boolean)
      .map(Number)
      .filter(Number.isInteger);
  } catch (error) {
    if (error && error.status === 1) return [];
    throw error;
  }
}

function isAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (_) {
    return false;
  }
}

async function waitForChild(parentPid) {
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) {
    const pids = childPids(parentPid);
    if (pids.length === 1) return pids[0];
    if (pids.length > 1) throw new Error(`launcher ${parentPid} has multiple children: ${pids}`);
    await sleep(50);
  }
  throw new Error(`launcher ${parentPid} never started the native child`);
}

async function waitForExit(pid, label, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (!isAlive(pid)) return;
    await sleep(50);
  }
  throw new Error(`${label} process ${pid} was left running`);
}

function readJsonLine(lines, label) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`${label} timed out`)), 10000);
    lines.once('line', (line) => {
      clearTimeout(timer);
      try {
        resolve(JSON.parse(line));
      } catch (error) {
        reject(new Error(`${label} returned invalid JSON: ${error.message}`));
      }
    });
  });
}

function waitForHandleExit(processHandle, label, timeoutMs = 10000) {
  if (processHandle.exitCode !== null || processHandle.signalCode !== null) {
    return Promise.resolve({ code: processHandle.exitCode, signal: processHandle.signalCode });
  }
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`${label} process ${processHandle.pid} was left running`)), timeoutMs);
    processHandle.once('exit', (code, signal) => {
      clearTimeout(timer);
      resolve({ code, signal });
    });
  });
}

async function startMcp(args = ['serve']) {
  const processHandle = spawn(process.execPath, [launcher, ...args], {
    stdio: ['pipe', 'pipe', 'pipe'],
    windowsHide: true,
  });
  const lines = readline.createInterface({ input: processHandle.stdout });
  try {
    const response = readJsonLine(lines, 'MCP initialize');
    processHandle.stdin.write(
      `${JSON.stringify({
        jsonrpc: '2.0',
        id: 1,
        method: 'initialize',
        params: {
          protocolVersion: '2025-06-18',
          capabilities: {},
          clientInfo: { name: 'launcher-lifecycle', version: '1.0' },
        },
      })}\n`,
    );
    const initialized = await response;
    if (initialized.id !== 1 || initialized.error) {
      throw new Error(`MCP initialize failed: ${JSON.stringify(initialized)}`);
    }
    const nativePid = await waitForChild(processHandle.pid);
    return { processHandle, nativePid, lines };
  } catch (error) {
    lines.close();
    processHandle.kill('SIGKILL');
    throw error;
  }
}

async function assertHardParentDeathClosesNativeChild() {
  const { processHandle, nativePid, lines } = await startMcp();
  lines.close();
  processHandle.kill('SIGKILL');
  await waitForHandleExit(processHandle, 'launcher');
  await waitForExit(nativePid, 'native child after launcher death', SIGNAL_FORWARD_DEADLINE_MS);
}

async function assertSignalIsForwarded(signal) {
  const { processHandle, nativePid, lines } = await startMcp();
  lines.close();
  const started = Date.now();
  processHandle.kill(signal);
  await waitForHandleExit(processHandle, `launcher after ${signal}`);
  await waitForExit(nativePid, `native child after ${signal}`);
  const elapsed = Date.now() - started;
  if (elapsed >= SIGNAL_FORWARD_DEADLINE_MS) {
    throw new Error(`${signal} took ${elapsed} ms; the native child appears to have required the launcher's force-kill fallback`);
  }
}

async function assertStdinEofClosesNativeChild() {
  const { processHandle, nativePid, lines } = await startMcp();
  lines.close();
  processHandle.stdin.end();
  const exit = await waitForHandleExit(processHandle, 'launcher after stdin EOF', SIGNAL_FORWARD_DEADLINE_MS);
  await waitForExit(nativePid, 'native child after stdin EOF', SIGNAL_FORWARD_DEADLINE_MS);
  if (exit.code !== 0 || exit.signal) {
    throw new Error(`launcher did not exit cleanly after stdin EOF: ${JSON.stringify(exit)}`);
  }
}

async function assertMcpTools(args, expectedTools) {
  const started = await startMcp(args);
  try {
    const listedResponse = readJsonLine(started.lines, `MCP tools/list for ${args.join(' ')}`);
    started.processHandle.stdin.write(
      `${JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized', params: {} })}\n`,
    );
    started.processHandle.stdin.write(
      `${JSON.stringify({ jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} })}\n`,
    );
    const listed = await listedResponse;
    if (listed.id !== 2 || listed.error) {
      throw new Error(`MCP tools/list failed for ${args.join(' ')}: ${JSON.stringify(listed)}`);
    }
    const actual = listed.result.tools.map((tool) => tool.name).sort();
    const expected = [...expectedTools].sort();
    if (JSON.stringify(actual) !== JSON.stringify(expected)) {
      throw new Error(`MCP tools/list mismatch for ${args.join(' ')}: expected ${expected}; got ${actual}`);
    }
    started.lines.close();
    started.processHandle.stdin.end();
    const exit = await waitForHandleExit(started.processHandle, `launcher for ${args.join(' ')}`);
    await waitForExit(started.nativePid, `native child for ${args.join(' ')}`);
    if (exit.code !== 0 || exit.signal) {
      throw new Error(`launcher did not exit cleanly for ${args.join(' ')}: ${JSON.stringify(exit)}`);
    }
  } catch (error) {
    started.lines.close();
    started.processHandle.kill('SIGKILL');
    throw error;
  }
}

function linkOrCopyExecutable(source, target) {
  try {
    fs.linkSync(source, target);
  } catch (_) {
    fs.copyFileSync(source, target);
  }
  if (process.platform !== 'win32') fs.chmodSync(target, 0o755);
}

function assertMissingPlatformPackageUsesStableCopyOrGivesAnActionableExit() {
  const targets = {
    'win32-x64': ['@fastctx/win32-x64', 'fastctx.exe'],
    'linux-x64': ['@fastctx/linux-x64', 'fastctx'],
    'darwin-x64': ['@fastctx/darwin-x64', 'fastctx'],
    'darwin-arm64': ['@fastctx/darwin-arm64', 'fastctx'],
  };
  const target = targets[`${process.platform}-${process.arch}`];
  if (!target) return;
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'fastctx-platform-fallback-'));
  try {
    const inputLauncher = fs.readFileSync(launcher, 'utf8');
    const mainLauncher = inputLauncher.includes("require('fastctx/launcher.js')")
      ? require.resolve('fastctx/launcher.js', { paths: [path.dirname(launcher)] })
      : launcher;
    const packageRoot = path.join(workspace, 'node_modules', 'fastctx');
    fs.mkdirSync(packageRoot, { recursive: true });
    const fixtureLauncher = path.join(packageRoot, 'launcher.js');
    fs.copyFileSync(mainLauncher, fixtureLauncher);

    const installedPlatformRoot = path.dirname(
      require.resolve(`${target[0]}/package.json`, { paths: [path.dirname(mainLauncher)] }),
    );
    const installedExecutable = path.join(installedPlatformRoot, 'bin', target[1]);
    const fixtureHome = path.join(workspace, 'home');
    const stableExecutable = path.join(fixtureHome, '.fastctx', 'bin', target[1]);
    fs.mkdirSync(path.dirname(stableExecutable), { recursive: true });
    linkOrCopyExecutable(installedExecutable, stableExecutable);
    const environment = {
      ...process.env,
      HOME: process.platform === 'win32' ? path.join(workspace, 'stale-home') : fixtureHome,
      USERPROFILE: fixtureHome,
    };
    const assertFallback = (label) => {
      const fallback = spawnSync(process.execPath, [fixtureLauncher, '--version'], {
        encoding: 'utf8',
        env: environment,
        windowsHide: true,
      });
      if (fallback.status !== 0 || !fallback.stdout.includes('fastctx ')) {
        throw new Error(`${label} stable-copy fallback failed: ${fallback.stderr || fallback.error || ''}`);
      }
      if (
        !fallback.stderr.includes(`platform package ${target[0]} is missing; using the stable copy`) ||
        !fallback.stderr.includes(stableExecutable)
      ) {
        throw new Error(`${label} stable-copy fallback omitted its warning: ${fallback.stderr}`);
      }
    };

    assertFallback('missing package');
    const malformedPackageRoot = path.join(workspace, 'node_modules', ...target[0].split('/'));
    fs.mkdirSync(malformedPackageRoot, { recursive: true });
    fs.writeFileSync(
      path.join(malformedPackageRoot, 'package.json'),
      `${JSON.stringify({ name: target[0], version: '0.1.0' }, null, 2)}\n`,
    );
    assertFallback('missing native executable');

    fs.unlinkSync(stableExecutable);
    const unavailable = spawnSync(process.execPath, [fixtureLauncher, '--version'], {
      encoding: 'utf8',
      env: environment,
      windowsHide: true,
    });
    if (unavailable.status !== 1) {
      throw new Error(`double-missing launcher exited ${unavailable.status}`);
    }
    for (const expected of [
      `platform package ${target[0]} is missing`,
      'registry may not have synchronized',
      'npm install --global fastctx --registry=https://registry.npmjs.org/',
    ]) {
      if (!unavailable.stderr.includes(expected)) {
        throw new Error(`double-missing launcher omitted ${expected}: ${unavailable.stderr}`);
      }
    }
  } finally {
    fs.rmSync(workspace, { recursive: true, force: true });
  }
}

function assertUpdateHandoffKeepsLauncherAlive() {
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'fastctx-handoff-'));
  try {
    const packageRoot = path.join(workspace, 'node_modules', 'fastctx');
    fs.mkdirSync(packageRoot, { recursive: true });
    const inputLauncher = fs.readFileSync(launcher, 'utf8');
    const isAlias = inputLauncher.includes("require('fastctx/launcher.js')");
    const mainLauncher = isAlias
      ? require.resolve('fastctx/launcher.js', { paths: [path.dirname(launcher)] })
      : launcher;
    const fixtureMainLauncher = path.join(packageRoot, 'launcher.js');
    fs.copyFileSync(mainLauncher, fixtureMainLauncher);
    let fixtureLauncher = fixtureMainLauncher;
    if (isAlias) {
      const aliasRoot = path.join(workspace, 'node_modules', 'codex-fastctx');
      fs.mkdirSync(aliasRoot, { recursive: true });
      fixtureLauncher = path.join(aliasRoot, 'launcher.js');
      fs.copyFileSync(launcher, fixtureLauncher);
    }
    const expectedPackage = isAlias ? 'codex-fastctx' : 'fastctx';
    const targets = {
      'win32-x64': ['@fastctx/win32-x64', 'fastctx.exe'],
      'linux-x64': ['@fastctx/linux-x64', 'fastctx'],
      'darwin-x64': ['@fastctx/darwin-x64', 'fastctx'],
      'darwin-arm64': ['@fastctx/darwin-arm64', 'fastctx'],
    };
    const target = targets[`${process.platform}-${process.arch}`];
    if (!target) return;
    const platformRoot = path.join(workspace, 'node_modules', target[0]);
    const binRoot = path.join(platformRoot, 'bin');
    fs.mkdirSync(binRoot, { recursive: true });
    fs.writeFileSync(
      path.join(platformRoot, 'package.json'),
      JSON.stringify({ name: target[0], version: '0.0.0' }),
    );
    linkOrCopyExecutable(process.execPath, path.join(binRoot, target[1]));
    const fixtureHome = path.join(workspace, 'home');
    fs.mkdirSync(fixtureHome, { recursive: true });
    const provenanceNames = [
      'FASTCTX_NPM_LAUNCHER_VERSION',
      'FASTCTX_NPM_PACKAGE',
      'FASTCTX_NPM_MODE',
      'FASTCTX_NODE_EXECUTABLE',
      'FASTCTX_NPM_CLI',
      'FASTCTX_NPM_LAUNCHER',
      'FASTCTX_NPM_LAUNCHER_PID',
      'FASTCTX_NPM_HANDOFF',
    ];
    const nonTui = spawnSync(
      process.execPath,
      [
        fixtureLauncher,
        '-e',
        `process.exit(${JSON.stringify(provenanceNames)}.some((name) => process.env[name]) ? 9 : 0)`,
      ],
      {
        env: Object.fromEntries([
          ...Object.entries(process.env),
          ...provenanceNames.map((name) => [name, 'must-not-leak']),
        ]),
        windowsHide: true,
      },
    );
    if (nonTui.status !== 0) {
      throw new Error('npm update provenance leaked into a non-TUI native process');
    }
    const nonTuiPrivateCode = spawnSync(
      process.execPath,
      [fixtureLauncher, '-e', 'process.exit(75)'],
      { windowsHide: true },
    );
    if (nonTuiPrivateCode.status !== 75) {
      throw new Error(
        `non-TUI exit code 75 was mistaken for an update handoff: got ${nonTuiPrivateCode.status}`,
      );
    }
    const helperScript = path.join(workspace, 'handoff-helper.js');
    fs.writeFileSync(
      helperScript,
      `'use strict';
const fs = require('node:fs');
setTimeout(() => {
  fs.writeFileSync(process.argv[2], JSON.stringify({
    schema_version: 1,
    state: 'done',
    helper_pid: process.pid,
    helper_executable: process.argv[3],
  }));
  process.exit(0);
}, 350);
`,
    );
    const bootstrap = path.join(workspace, 'handoff-bootstrap.js');
    fs.writeFileSync(
      bootstrap,
      `'use strict';
if (process.env.FASTCTX_NPM_LAUNCHER_VERSION === '1') {
  const { spawn } = require('node:child_process');
  const fs = require('node:fs');
  const path = require('node:path');
  const handoff = process.env.FASTCTX_NPM_HANDOFF;
  const helperExecutable = process.env.FASTCTX_HANDOFF_FIXTURE_HELPER;
  for (const name of [
    'FASTCTX_NPM_PACKAGE',
    'FASTCTX_NPM_MODE',
    'FASTCTX_NODE_EXECUTABLE',
    'FASTCTX_NPM_CLI',
    'FASTCTX_NPM_LAUNCHER',
    'FASTCTX_NPM_LAUNCHER_PID',
    'FASTCTX_NPM_HANDOFF',
  ]) {
    if (!process.env[name]) throw new Error('launcher omitted ' + name);
  }
  if (process.env.FASTCTX_NPM_MODE !== 'exec') {
    throw new Error('expected exec provenance');
  }
  if (process.env.FASTCTX_NPM_PACKAGE !== process.env.FASTCTX_HANDOFF_EXPECTED_PACKAGE) {
    throw new Error('launcher reported the wrong npm package');
  }
  fs.mkdirSync(path.dirname(handoff), { recursive: true });
  if (process.env.FASTCTX_HANDOFF_FIXTURE_BEHAVIOR === 'missing') {
    process.exit(75);
  }
  fs.writeFileSync(helperExecutable, 'cleanup fixture');
  let helperPid = 0x7ffffffe;
  if (process.env.FASTCTX_HANDOFF_FIXTURE_BEHAVIOR === 'success') {
    const helperEnvironment = { ...process.env };
    delete helperEnvironment.NODE_OPTIONS;
    delete helperEnvironment.FASTCTX_NPM_LAUNCHER_VERSION;
    const helper = spawn(
      process.execPath,
      [process.env.FASTCTX_HANDOFF_FIXTURE_HELPER_SCRIPT, handoff, helperExecutable],
      { detached: true, stdio: 'ignore', windowsHide: true, env: helperEnvironment },
    );
    helper.unref();
    helperPid = helper.pid;
  }
  fs.writeFileSync(handoff, JSON.stringify({
    schema_version: 1,
    state: 'running',
    helper_pid: helperPid,
    helper_executable: helperExecutable,
  }));
  process.exit(75);
}
`,
    );
    const outerBootstrap = `
Object.defineProperty(process.stdin, 'isTTY', { value: true, configurable: true });
Object.defineProperty(process.stdout, 'isTTY', { value: true, configurable: true });
process.argv = [process.execPath, ${JSON.stringify(fixtureLauncher)}];
require(${JSON.stringify(fixtureLauncher)});
`;

    for (const behavior of ['success', 'failure', 'missing']) {
      const helperExecutable = path.join(workspace, `helper-${behavior}`);
      const started = Date.now();
      const result = spawnSync(
        process.execPath,
        ['-e', outerBootstrap],
        {
          encoding: 'utf8',
          env: {
            ...process.env,
            HOME: fixtureHome,
            USERPROFILE: fixtureHome,
            npm_command: 'exec',
            NODE_OPTIONS: `--require="${bootstrap.replace(/\\/g, '/')}"`,
            FASTCTX_HANDOFF_FIXTURE_BEHAVIOR: behavior,
            FASTCTX_HANDOFF_FIXTURE_HELPER: helperExecutable,
            FASTCTX_HANDOFF_FIXTURE_HELPER_SCRIPT: helperScript,
            FASTCTX_HANDOFF_EXPECTED_PACKAGE: expectedPackage,
          },
          timeout: 10000,
          windowsHide: true,
        },
      );
      const elapsed = Date.now() - started;
      const expectedStatus = behavior === 'success' ? 0 : 1;
      if (result.status !== expectedStatus) {
        throw new Error(
          `${behavior} update handoff exited ${result.status}: ${result.stderr || result.error || ''}`,
        );
      }
      if (behavior === 'success' && elapsed < 300) {
        throw new Error(`npm launcher returned before the updater session ended (${elapsed} ms)`);
      }
      if (fs.existsSync(helperExecutable)) {
        throw new Error(`${behavior} update helper was not cleaned up`);
      }
    }
  } finally {
    fs.rmSync(workspace, { recursive: true, force: true });
  }
}

async function main() {
  assertMissingPlatformPackageUsesStableCopyOrGivesAnActionableExit();
  assertUpdateHandoffKeepsLauncherAlive();
  const invalid = spawnSync(process.execPath, [launcher, '__invalid_subcommand__'], {
    encoding: 'utf8',
    windowsHide: true,
  });
  if (invalid.status !== 2) {
    throw new Error(`native exit code was not preserved: expected 2, got ${invalid.status}`);
  }
  await assertMcpTools(['serve'], ['read', 'grep', 'glob', 'replace']);
  await assertMcpTools(
    ['serve', '--enable-shell'],
    ['read', 'grep', 'glob', 'replace', 'run', 'run_background', 'job_output', 'job_kill', 'job_list'],
  );
  await assertMcpTools(
    ['serve', '--enable-edit'],
    ['read', 'grep', 'glob', 'replace'],
  );
  await assertMcpTools(
    ['serve', '--enable-shell', '--enable-edit'],
    ['read', 'grep', 'glob', 'replace', 'run', 'run_background', 'job_output', 'job_kill', 'job_list'],
  );
  await assertStdinEofClosesNativeChild();
  await assertHardParentDeathClosesNativeChild();
  await assertSignalIsForwarded('SIGINT');
  await assertSignalIsForwarded('SIGTERM');
}

main().catch((error) => {
  console.error(error.stack || error.message || String(error));
  process.exit(1);
});
