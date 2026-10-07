let built;
export const buildSupport = (...args) => buildSupportWithEnv({}, ...args);
export async function buildSupportWithEnv(env, ...args) {
  built ??= (async () => {
    const build = Bun.spawn(['cargo', 'build', '--locked', '--example', 'build-support'], { cwd: new URL('../../', import.meta.url).pathname, stdout: 'ignore', stderr: 'pipe' });
    const diagnostic = await new Response(build.stderr).text();
    if (await build.exited) throw new Error(diagnostic);
  })();
  await built;
  const child = Bun.spawn([new URL('../../target/debug/examples/build-support', import.meta.url).pathname, ...args], { stdout: 'pipe', stderr: 'pipe', env: { ...process.env, ...env } });
  const [out, err, code] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  if (code) throw new Error(err.trim());
  return JSON.parse(out);
}
