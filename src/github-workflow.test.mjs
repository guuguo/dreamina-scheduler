import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import assert from 'node:assert/strict';

const workflowText = readFileSync('.github/workflows/build.yml', 'utf8');
const packageVersion = JSON.parse(readFileSync('package.json', 'utf8')).version;

function extractReadPackageVersionScript() {
  const match = workflowText.match(/- name: Read package version[\s\S]*?\n\s*run:\s*(?:\|\n(?<block>(?: {10}.+\n?)+)|(?<inline>.*))/);
  assert.ok(match, 'Read package version step should exist');
  if (match.groups?.block) {
    return match.groups.block
      .split('\n')
      .map((line) => line.replace(/^ {8}/, ''))
      .join('\n')
      .trim();
  }
  return match.groups.inline.trim();
}

test('GitHub Actions package version step writes package version in bash', () => {
  const script = extractReadPackageVersionScript();
  const outputFile = join(mkdtempSync(join(tmpdir(), 'dreamina-actions-')), 'github-output');

  try {
    execFileSync('bash', ['-lc', script], {
      cwd: process.cwd(),
      env: { ...process.env, GITHUB_OUTPUT: outputFile },
      stdio: 'pipe',
    });

    assert.equal(readFileSync(outputFile, 'utf8').trim(), `version=${packageVersion}`);
  } finally {
    rmSync(outputFile, { force: true });
  }
});
