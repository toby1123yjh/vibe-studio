import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { runInNewContext } from 'node:vm';

const workflow = readFileSync(
  new URL('../../.github/workflows/publish-easy-npx.yml', import.meta.url),
  'utf8'
);
const autoVersionBase = workflow.match(/AUTO_VERSION_BASE:\s*([^\r\n]+)/)?.[1]?.trim();
assert.equal(autoVersionBase, '2.0.0-beta.1');
const validationStep = workflow.split(
  '- name: Validate requested npm version'
)[1]?.split('\n      - name:')[0];
const snippet = validationStep?.match(/node -e "\r?\n([\s\S]*?)\r?\n\s*"/)[1];
assert.ok(snippet, 'Workflow must contain the npm version validation snippet');

function validate(version) {
  runInNewContext(snippet, { process: { env: { VERSION: version } } });
}

const resolver = workflow.match(/node <<'NODE'\r?\n([\s\S]*?)\r?\n\s*NODE/)[1];

function resolveRelease({ versions = [], lookupError, env = {}, name = '@toby1123yjh/vibe-studio' } = {}) {
  let output = '';
  const lookups = [];
  runInNewContext(resolver, {
    require(module) {
      if (module === 'node:fs') {
        return {
          readFileSync: () => JSON.stringify({ name, version: '2.0.0-beta.1' }),
          appendFileSync: (_path, value) => { output += value; },
        };
      }
      if (module === 'node:child_process') {
        return {
          execFileSync(command, args) {
            lookups.push([command, ...args]);
            if (lookupError) throw lookupError;
            return JSON.stringify(versions);
          },
        };
      }
      throw new Error(`Unexpected module: ${module}`);
    },
    process: { env: { AUTO_VERSION_BASE: autoVersionBase, AUTO_NPM_TAG: 'beta', EVENT_NAME: 'push', ...env } },
    console: { log() {} },
  });
  return { output: Object.fromEntries(output.trim().split('\n').map((line) => line.split('='))), lookups };
}

test('auto version uses the metadata package and the greatest version in its beta line', () => {
  const { output, lookups } = resolveRelease({
    name: 'another-distribution',
    versions: ['0.1.44-easy.90', '2.0.0-beta.2', '2.0.0-beta.10', '3.0.0-beta.1'],
  });
  assert.equal(lookups[0][2], 'another-distribution');
  assert.equal(output.package_name, 'another-distribution');
  assert.equal(output.version, '2.0.0-beta.11');
  assert.equal(output.npm_tag, 'beta');
  assert.equal(output.package_exists, 'true');
});

test('a genuine npm E404 starts a new package at the configured baseline', () => {
  const lookupError = Object.assign(new Error('not found'), {
    stdout: JSON.stringify({ error: { code: 'E404' } }),
  });
  const { output, lookups } = resolveRelease({ lookupError });
  assert.equal(lookups[0][2], '@toby1123yjh/vibe-studio');
  assert.equal(output.package_name, '@toby1123yjh/vibe-studio');
  assert.equal(output.version, '2.0.0-beta.1');
  assert.equal(output.package_exists, 'false');
});

test('scoped packages increment their own published versions', () => {
  const { output, lookups } = resolveRelease({ versions: ['2.0.0-beta.1'] });
  assert.equal(lookups[0][2], '@toby1123yjh/vibe-studio');
  assert.equal(output.package_name, '@toby1123yjh/vibe-studio');
  assert.equal(output.version, '2.0.0-beta.2');
});

test('built artifacts are saved before publishing with a filesystem-safe name', () => {
  const pack = workflow.indexOf('- name: Build and pack NPX package');
  const upload = workflow.indexOf('- name: Upload package artifacts');
  const publish = workflow.indexOf('- name: Publish package to npm');
  assert.ok(pack >= 0 && pack < upload && upload < publish);
  const uploadStep = workflow.slice(upload, publish);
  const artifactName = uploadStep.match(/\n\s+name: (.+)/)?.[1]
    .replace('${{ needs.resolve-version.outputs.version }}', '2.0.0-beta.1');
  assert.ok(artifactName);
  assert.doesNotMatch(artifactName, /["<>|:*?\\/\r\n${}]/);
});

test('registry network and authentication failures do not reset the version', () => {
  for (const code of ['E401', 'ETIMEDOUT', 'E500']) {
    const lookupError = Object.assign(new Error(code), {
      stdout: JSON.stringify({ error: { code } }),
    });
    assert.throws(() => resolveRelease({ lookupError }), /Cannot read published versions/);
  }
  assert.throws(() => resolveRelease({ versions: { error: 'invalid response' } }), /Cannot read published versions/);
});

test('explicit already published version is rejected before builds', () => {
  assert.throws(() => resolveRelease({
    versions: ['2.0.0-beta.1'],
    env: { REQUESTED_VERSION: '2.0.0-beta.1' },
  }), /vibe-studio@2\.0\.0-beta\.1 already exists/);
});

test('manual dry run preserves the requested version and tag', () => {
  const { output } = resolveRelease({ env: {
    EVENT_NAME: 'workflow_dispatch',
    REQUESTED_VERSION: '2.0.0',
    REQUESTED_NPM_TAG: 'latest',
    REQUESTED_PUBLISH_MODE: 'dry-run',
  } });
  assert.equal(output.version, '2.0.0');
  assert.equal(output.npm_tag, 'latest');
  assert.equal(output.publish_mode, 'dry-run');
});

for (const version of [
  '2.0.0',
  '2.0.0-beta.1',
  '0.1.44-easy.1',
  '0.0.0',
  '10.20.30',
  '2.0.0-beta.0',
  '0.1.44-easy.64',
]) {
  test(`accepts ${version}`, () => {
    assert.doesNotThrow(() => validate(version));
  });
}

for (const version of [
  undefined,
  '',
  '2.0',
  'v2.0.0',
  '02.0.0',
  '2.00.0',
  '2.0.00',
  '2.0.0-beta.01',
  '2.0.0-easy.01',
  '2.0.0-beta',
  '2.0.0-beta.',
  '2.0.0-beta.-1',
  '2.0.0-alpha.1',
  '2.0.0-beta.1.extra',
  '2.0.0+build.1',
  ' 2.0.0',
  '2.0.0 ',
  '2.0.0\n',
  '2.0.0-beta.1\r\n',
  '2x0x0',
]) {
  test(`rejects ${JSON.stringify(version)}`, () => {
    assert.throws(() => validate(version), /Version must be X\.Y\.Z/);
  });
}
