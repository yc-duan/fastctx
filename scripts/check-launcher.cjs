'use strict';

// Exercise the published wrapper contract with real child processes.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

assert(process.argv[2], 'Supply a directory for disposable test files');
const root = fs.mkdtempSync(path.join(path.resolve(process.argv[2]), 'launcher-contract-'));
try {
  const launcher = path.join(root, 'launcher.js');
  fs.copyFileSync(path.join(__dirname, '../packages/launcher.js'), launcher);
  fs.writeFileSync(path.join(root, 'package.json'), JSON.stringify({ version: '1.0.0' }));
  const platform = path.join(root, 'node_modules/@fastctx', `${process.platform}-${process.arch}`);
  fs.mkdirSync(path.join(platform, 'bin'), { recursive: true });
  fs.writeFileSync(path.join(platform, 'package.json'), JSON.stringify({ version: '1.0.0' }));
  const binary = path.join(platform, 'bin', `fastctx${process.platform === 'win32' ? '.exe' : ''}`);
  fs.copyFileSync(process.execPath, binary);
  const probe = path.join(root, 'probe.cjs');
  fs.writeFileSync(probe, `
    const fs = require('node:fs');
    if (process.env.PROBE_SIGNAL) process.kill(process.pid, 'SIGTERM');
    else {
      process.stdout.write(JSON.stringify({args: process.argv.slice(2),
        input: fs.readFileSync(0, 'utf8'), handover: process.env.FASTCTX_UPDATE_FINALIZE,
        cwd: process.cwd()}));
      process.stderr.write('native stderr');
      process.exit(Number(process.env.PROBE_EXIT));
    }
  `);
  const args = ['space in argument', '中文', '"quote"', 'back\\slash', '--', ''];
  for (const code of [0, 7, 75]) {
    const child = spawnSync(process.execPath, [launcher, probe, ...args], {
      cwd: root, input: 'stdin bytes\n', encoding: 'utf8',
      env: { ...process.env, PROBE_EXIT: String(code), FASTCTX_UPDATE_FINALIZE: path.join(root, 'request.json') },
    });
    assert.ifError(child.error);
    assert.equal(child.status, code);
    assert.equal(child.stderr, 'native stderr');
    assert.deepEqual(JSON.parse(child.stdout), {
      args, input: 'stdin bytes\n', handover: path.join(root, 'request.json'), cwd: root,
    });
  }
  if (process.platform !== 'win32') {
    const child = spawnSync(process.execPath, [launcher, probe], {
      env: { ...process.env, PROBE_SIGNAL: '1' },
    });
    assert.equal(child.signal, 'SIGTERM');
  }
  fs.writeFileSync(path.join(platform, 'package.json'), JSON.stringify({ version: '0.2.6' }));
  const mismatch = spawnSync(process.execPath, [launcher, probe], { encoding: 'utf8' });
  assert.equal(mismatch.status, 1);
  assert.match(mismatch.stderr, /requires 1\.0\.0/);
  console.log('Launcher contract passed: argv, environment, cwd, stdio, exit codes, version matching');
} finally {
  fs.rmSync(root, { recursive: true });
}
