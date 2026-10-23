import { test, expect } from 'bun:test';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { baseVersion } from '../scripts/version.js';
const scheduler = new URL('../scripts/prepare-nightly.js', import.meta.url).pathname;
async function fixture(fn) {
  const root = await mkdtemp(join(tmpdir(), 'distribution-scheduler-'));
  try {
    for (const name of ['bin', 'release', 'scripts']) await mkdir(join(root, name));
    const git = (...args) => {
      const p = Bun.spawnSync(['git', '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', ...args], {cwd:root});
      if (p.exitCode) throw new Error(p.stderr.toString());
      return p.stdout.toString().trim();
    };
    git('init', '-q'); git('commit', '--allow-empty', '-qm', 'reviewed source');
    const revision = git('rev-parse', 'HEAD');
    const preload = join(root, 'transport.js');
    await writeFile(preload, `globalThis.fetch = async url => {
      if (url !== 'https://downloads.apeironlab.cn/apeiron-cli/channels/stable.txt') throw new Error('unexpected URL');
      return process.env.STABLE ? new Response(process.env.STABLE) : new Response('', {status:404});
    };`);
    await writeFile(join(root,'scripts/check-distribution-source.js'), `if(process.env.FAIL_PINS) process.exit(1);`);
    await writeFile(join(root,'bin/gh'), `#!${process.execPath}
      import {appendFile,readFile,writeFile} from 'node:fs/promises';
      const a=process.argv.slice(2), root=process.env.FIXTURE;
      await appendFile(root+'/calls',JSON.stringify(a)+'\\n');
      if(a[0]==='pr') console.log(process.env.PENDING || '[]');
      else if(a[0]==='run') console.log(process.env.RUNS || '[]');
      else if(a.includes('POST')) await writeFile(root+'/tag',JSON.stringify({object:{type:'commit',sha:a.find(x=>x.startsWith('sha=')).slice(4)}}));
      else if(a[0]==='api') {
        try {console.log(await readFile(root+'/tag','utf8'));}
        catch {console.error('HTTP 404');process.exit(1);}
      } else if(a[0]==='workflow' && process.env.FAIL_DISPATCH) process.exit(1);
    `,{mode:0o755});
    const run=async(extra={})=>{
      const p=Bun.spawn([process.execPath,'--preload',preload,scheduler],{cwd:root,stdout:'pipe',stderr:'pipe',env:{...process.env,
        PATH:join(root,'bin')+':'+process.env.PATH,FIXTURE:root,GITHUB_REPOSITORY:'TheApeironLab/apeiron-cli',GITHUB_REF:'refs/heads/main',GITHUB_SHA:revision,...extra}});
      const [code,out,err]=await Promise.all([p.exited,new Response(p.stdout).text(),new Response(p.stderr).text()]);
      return {code,out,err};
    };
    const log=async()=>await Bun.file(join(root,'calls')).exists()?(await readFile(join(root,'calls'),'utf8')).trim().split('\n').map(JSON.parse):[];
    await fn({root,run,log,revision});
  } finally {await rm(root,{recursive:true,force:true});}
}
test('nightly prepares one complete distribution, preserves pending pin reviews and stops after stable',()=>fixture(async({run,log})=>{
  expect((await run()).code).toBe(0);
  expect((await log()).at(-1)).toEqual(['workflow','run','prepare-distribution.yml','--repo','TheApeironLab/apeiron-cli','--ref','main','-f','channel=nightly']);
  const writes=(await log()).filter(a=>a[0]==='workflow').length;
  expect((await run({PENDING:JSON.stringify([{headRefName:'codex/distribution-0.2.1-nightly.20261023.1'}])})).out).toContain('awaiting review');
  expect((await run({STABLE:baseVersion})).out).toContain('Advance Cargo');
  expect((await log()).filter(a=>a[0]==='workflow')).toHaveLength(writes);
}));
test('merged nightly pins retain their identity, recover a failed dispatch, and reuse the original run',()=>fixture(async({root,run,log,revision})=>{
  const version=baseVersion+'-nightly.20261023.123';
  await writeFile(join(root,'release/distribution-input.json'),JSON.stringify({version}));
  expect((await run({FAIL_PINS:'1'})).code).not.toBe(0);
  expect((await log()).some(a=>a.includes('POST'))).toBe(false);
  expect((await run({FAIL_DISPATCH:'1'})).code).not.toBe(0);
  expect(JSON.parse(await readFile(join(root,'tag'),'utf8')).object.sha).toBe(revision);
  expect((await run()).code).toBe(0);
  expect((await log()).filter(a=>a.includes('POST'))).toHaveLength(1);
  expect((await log()).at(-1)).toEqual(['workflow','run','release.yml','--repo','TheApeironLab/apeiron-cli','--ref','v'+version]);
  expect((await run({RUNS:'[{"databaseId":42}]'})).out).toContain('original workflow');
}));
test('foreign branches and corrupt stable pointers never create tags or builds',()=>fixture(async({run,log})=>{
  expect((await run({GITHUB_REF:'refs/heads/feature'})).code).not.toBe(0);
  expect((await run({STABLE:'../unexpected'})).code).not.toBe(0);
  expect(await log()).toEqual([]);
}));
