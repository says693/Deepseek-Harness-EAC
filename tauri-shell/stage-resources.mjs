import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const shellDir = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(shellDir, '..');
const stage = path.join(repo, 'tauri-app', 'scripts', 'stage.ts');

console.log('[stage-resources] AIO compatibility entry → tauri-app/scripts/stage.ts');
const result = spawnSync(process.execPath, [stage], {
  cwd: repo,
  env: process.env,
  stdio: 'inherit',
});
if (result.error) throw result.error;
process.exit(result.status == null ? 1 : result.status);
