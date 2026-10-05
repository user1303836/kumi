#!/usr/bin/env node
// Compatibility for existing npm commands. Application behavior lives in the Rust binaries.
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const setup = process.argv[2] === '--setup';
const args = setup ? [] : process.argv.slice(2);
let failureMessage = 'Kumi could not start. Check the native installation or install Rust to build this checkout.';
const run = (command, argv, env = process.env) => {
  const result = spawnSync(command, argv, { cwd: root, stdio: 'inherit', env });
  if (result.error) throw result.error;
  if (result.signal) process.kill(process.pid, result.signal);
  return result.status ?? 1;
};
try {
  if (spawnSync('cargo', ['--version'], { stdio: 'ignore' }).status === 0) {
    const env = { ...process.env };
    delete env.KUMI_INSTALLED;
    const built = run('cargo', ['build', '--quiet', '--release', '--locked', '--workspace', '--bins'], env);
    process.exitCode = built || (setup ? 0 : run('cargo', ['run', '--quiet', '--release', '--locked', '-p', 'kumi', '--', ...args], env));
  } else {
    const home = process.env.KUMI_HOME || join(homedir(), '.kumi');
    const executable = join(home, 'app', process.platform === 'win32' ? 'kumi.exe' : 'kumi');
    const env = { ...process.env, KUMI_HOME: home, KUMI_INSTALLED: '1' };
    if (setup || !existsSync(executable)) {
      const version = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')).version;
      // A checkout's version is preferable to silently changing branches or application versions.
      if (!env.KUMI_VERSION && !env.KUMI_RELEASES) env.KUMI_VERSION = version;
      console.log('Kumi is switching this npm installation to the native release. Your settings, sign-ins, conversations and library stay in the same Kumi home.');
      failureMessage = 'Kumi could not start the native installer. Check that a system shell is available.';
      const result = process.platform === 'win32'
        ? run(join(process.env.SystemRoot || 'C:\\Windows', 'System32/WindowsPowerShell/v1.0/powershell.exe'), ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', join(root, 'install.ps1')], env)
        : run('sh', [join(root, 'install.sh')], env);
      if (result !== 0) process.exit(result);
      if (!existsSync(executable)) {
        failureMessage = 'Kumi has no installed native executable. Use a published native release or install Rust to build this checkout.';
        throw new Error(failureMessage);
      }
    }
    failureMessage = 'Kumi could not start the installed native application. Run the installer to repair it.';
    process.exitCode = setup ? 0 : run(executable, args, env);
  }
} catch {
  // Spawn and filesystem errors can include private environment values and paths.
  console.error(failureMessage);
  process.exitCode = 1;
}
