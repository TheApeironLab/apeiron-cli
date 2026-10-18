import { test, expect } from 'bun:test';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { binary } from './support';

test('Task commands preserve project, column, field and relation HTTP contracts', async () => {
  const home = await mkdtemp(join(tmpdir(), 'task-contract-'));
  const seen = [];
  const server = Bun.serve({ hostname: '127.0.0.1', port: 0, async fetch(req) {
    const path = new URL(req.url).pathname;
    expect(req.headers.get('authorization')).toBe('Bearer fixture');
    seen.push({ method: req.method, path, body: ['POST','PUT'].includes(req.method) ? await req.json() : null });
    if (path.includes('missing')) return Response.json({message:'Workspace ID could not be determined'}, {status:400});
    if (path === '/api/task/bad-status') return Response.json({message:'Invalid status "private diagnostic". Valid statuses for this project: private'}, {status:400});
    if (path === '/api/task/required-field') return Response.json({message:'Custom field "private diagnostic" is required to create a task.'}, {status:400});
    if (req.method === 'GET') return Response.json([{ id:'r', slug:'to-do', sourceTaskId:'parent', targetTaskId:'child', relationType:'subtask', secret:'private', sourceTask:{id:'parent',secret:'private'} }]);
    return Response.json({id:'r',sourceTaskId:'parent',targetTaskId:'child',relationType:'subtask',dueDate:seen.at(-1).body?.dueDate??null,secret:'private'});
  }});
  const run = async (...args) => {
    const env = {...process.env, XDG_CONFIG_HOME:home, KANEO_API_URL:`http://127.0.0.1:${server.port}/api`, KANEO_ACCESS_TOKEN:'fixture'};
    for (const key of ['KANEO_ACCESS_TOKEN_FILE','KANEO_API_KEY','KANEO_API_KEY_FILE','KANEO_WORKSPACE_ID']) delete env[key];
    const p=Bun.spawn([binary,'task',...args,'--json'],{env,stdout:'pipe',stderr:'pipe'});
    const [out,err,code]=await Promise.all([new Response(p.stdout).text(),new Response(p.stderr).text(),p.exited]);
    expect(out+err).not.toContain('private');
    return {code,value:JSON.parse(out||err)};
  };
  try {
    const cases = [
      [['columns','--project','p'],'GET','/api/column/p'],
      [['project','columns','p'],'GET','/api/column/p'],
      [['project','fields','p'],'GET','/api/custom-field/project/p'],
      [['fields','t'],'GET','/api/custom-field/task/t'],
      [['relation','list','--task','t'],'GET','/api/task-relation/t'],
      [['relation','create','--source','parent','--target','child','--relation-type','subtask'],'POST','/api/task-relation'],
      [['relation','delete','r'],'DELETE','/api/task-relation/r'],
      [['project','delete','p','--confirm'],'DELETE','/api/project/p'],
      [['create','--project','p','--title','Test','--status','to-do'],'POST','/api/task/p'],
    ];
    for(const [args,method,path] of cases){
      expect((await run(...args)).code).toBe(0);
      expect(seen.at(-1).method).toBe(method);expect(seen.at(-1).path).toBe(path);
    }
    expect(seen.find(r=>r.path==='/api/task-relation'&&r.method==='POST').body).toEqual({sourceTaskId:'parent',targetTaskId:'child',relationType:'subtask'});
    expect(seen.at(-1).body).toEqual({title:'Test',status:'to-do',priority:'no-priority',description:'',customFields:[]});
    expect((await run('create','--project','p','--title','Test','--status','to-do','--start','2026-10-19T09:00:00+08:00','--due','2026-10-26')).code).toBe(0);
    expect(seen.at(-1).body.startDate).toBe('2026-10-19T09:00:00+08:00');
    expect(seen.at(-1).body.dueDate).toBe('2026-10-26T00:00:00Z');
    for(const flag of ['--due']) {
      for(const [date,expected] of [['2026-10-26','2026-10-26T00:00:00Z'],['2028-02-29','2028-02-29T00:00:00Z'],['2026-11-12T12:00:00+08:00','2026-11-12T12:00:00+08:00']]) {
        for(const [args,method,path] of [
          [['create','--project','p','--title','Milestone','--status','to-do',flag,date],'POST','/api/task/p'],
          [['update','t',flag,date],'PUT','/api/task/due-date/t'],
        ]) {
          const result=await run(...args);expect(result.code).toBe(0);expect(result.value.data.dueDate).toBe(expected);
          expect(seen.at(-1).method).toBe(method);expect(seen.at(-1).path).toBe(path);expect(seen.at(-1).body.dueDate).toBe(expected);
        }
      }
    }
    const file=join(home,'task.json');
    await writeFile(file,JSON.stringify({title:'Milestone',status:'to-do',description:'',priority:'no-priority',dueDate:'2026-11-12'}));
    expect((await run('create','--project','p','--file',file)).code).toBe(0);
    expect(seen.at(-1).body.dueDate).toBe('2026-11-12T00:00:00Z');
    expect((await run('update','t','--clear-due')).code).toBe(0);
    expect(seen.at(-1)).toEqual({method:'PUT',path:'/api/task/due-date/t',body:{}});
    const count=seen.length;
    expect((await run('update','t','--due-date','2026-10-26')).code).toBe(2);
    for(const date of ['2026-02-29','2026-13-01','10/26','2026-11-12T12:00:00','']) {
      for(const args of [['create','--project','p','--title','Test','--status','to-do'],['update','t']]) {
        const result=await run(...args,'--due',date);expect(result.code).toBe(2);expect(result.value.error.message).toContain('Due date');
      }
    }
    for(const args of [['update','t','--due','2026-10-26','--clear-due'],['create','--project','p','--file',file,'--due','2026-10-26']]) expect((await run(...args)).code).toBe(2);
    for(const args of [['project','delete','p'],['relation','create','--source','x','--target','x','--relation-type','subtask'],['relation','create','--source','x','--target','y','--relation-type','milestone']]) expect((await run(...args)).code).toBe(2);
    expect(seen.length).toBe(count);
    for(const [project,hint] of [['missing','actual IDs'],['bad-status','slug'],['required-field','--field']]){
      const result=project==='missing'?await run('project','columns',project):await run('create','--project',project,'--title','Test','--status','bad');
      expect(result.code).toBe(2);expect(result.value.error.code).toBe('INVALID_REQUEST');expect(result.value.error.message).toContain(hint);
    }
  } finally { server.stop(true); await rm(home,{recursive:true,force:true}); }
}, 30000);
