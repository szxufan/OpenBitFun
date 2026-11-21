import { createInterface } from 'node:readline';
import { createServer } from 'node:http';
import { appendFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';

const sessions = new Map(), active = new Map();
const event = (type, data = {}) => appendFileSync(process.env.OPENBITFUN_ACP_FIXTURE_LOG,
  JSON.stringify({ at: Date.now(), pid: process.pid, type, ...data }) + '\n');
const send = data => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', ...data }) + '\n');
const notify = (sessionId, text) => send({ method: 'session/update', params: { sessionId,
  update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text } } } });
const options = () => ({ modes: { currentModeId: 'default', availableModes: [{ id: 'default', name: 'Default' }] },
  models: { currentModelId: 'fixture', availableModels: [{ modelId: 'fixture', name: 'Controlled fixture' },
    { modelId: 'fixture-alt', name: 'Controlled alternative' }] } });
function finish(token, reason = 'end_turn', error = false) {
  const item = [...active.values()].find(item => item.token === token);
  if (!item) return false;
  clearInterval(item.timer); clearTimeout(item.auto); active.delete(item.sessionId);
  if (error) send({ id: item.id, error: { code: -32603, message: 'Controlled ACP transport error' } });
  else { notify(item.sessionId, `\nACP_COMPLETE:${token}\n`); send({ id: item.id, result: { stopReason: reason } }); }
  event('finished', { token, reason, error }); return true;
}
const control = createServer(async (request, response) => {
  if (request.method === 'GET') {
    response.end(JSON.stringify({ pid: process.pid, active: [...active.values()].map(({ token, sessionId, ticks }) => ({ token, sessionId, ticks })), sessions: [...sessions.keys()] })); return;
  }
  const parts=[]; for await(const part of request)parts.push(part);
  const body=JSON.parse(Buffer.concat(parts).toString());
  if (body.action === 'metadata') {
    const item = [...active.values()].find(item => item.token === body.token);
    if (item) {
      const update = update => send({ method: 'session/update', params: { sessionId: item.sessionId, update } });
      update({ sessionUpdate: 'available_commands_update', availableCommands: [
        { name: 'inspect', description: 'Inspect a path' },
      ] });
      update({ sessionUpdate: 'config_option_update', configOptions: [{
        id: 'reasoning', name: 'Reasoning', type: 'select', currentValue: 'high',
        options: [{ value: 'medium', name: 'Medium' }, { value: 'high', name: 'High' }],
      }] });
      update({ sessionUpdate: 'usage_update', used: 19, size: 1000 });
    }
    event('metadata', { token: body.token });
    response.end(JSON.stringify({ found: !!item })); return;
  }
  if(body.action==='pause'||body.action==='resume') {
    const item=[...active.values()].find(item=>item.token===body.token);
    if(item){clearInterval(item.timer);if(body.action==='resume')item.timer=setInterval(()=>{item.ticks++;notify(item.sessionId,` ${item.token}:${item.ticks}`)},180);}
    event(body.action,{token:body.token});response.end(JSON.stringify({found:!!item}));return;
  }
  if(body.action==='exit'){event('controlled_exit');response.end('{}');setTimeout(()=>process.exit(7),30);return;}
  response.end(JSON.stringify({ finished: finish(body.token, body.reason ?? 'end_turn',body.action==='error') }));
});
control.listen(0,'127.0.0.1',()=>event('started',{port:control.address().port}));
const lines=createInterface({input:process.stdin});
lines.on('line',line=>{
  let request;try{request=JSON.parse(line)}catch{return}
  const {id,method,params={}}=request;
  if(!method){event('client_reply',{id,result:request.result});return;}
  event('request',{method,id,sessionId:params.sessionId});
  if(method==='initialize'){send({id,result:{protocolVersion:1,agentInfo:{name:'OpenBitFun controlled ACP fixture',version:'1.0'},agentCapabilities:{loadSession:true,promptCapabilities:{image:false,audio:false,embeddedContext:false}},authMethods:[]}});return;}
  if(method==='session/new'){const sessionId='fixture-'+randomUUID();sessions.set(sessionId,{cwd:params.cwd});send({id,result:{sessionId,...options()}});return;}
  if(method==='session/load'){sessions.set(params.sessionId,{cwd:params.cwd});send({id,result:options()});return;}
  if(method==='session/prompt'){
    const text=(params.prompt??[]).filter(item=>item.type==='text').map(item=>item.text).join('\n');
    const token=[...text.matchAll(/ACP_CASE:([a-z0-9-]+)/g)].at(-1)?.[1]??'unmarked';
    if(active.has(params.sessionId)){send({id,error:{code:-32000,message:'Controlled fixture refuses overlapping prompts'}});return;}
    const item={id,sessionId:params.sessionId,token,ticks:0};active.set(params.sessionId,item);
    event('prompt',{token,sessionId:params.sessionId});notify(params.sessionId,`ACP_STARTED:${token}\n`);
    if (token === 'hold-before-refresh') {
      const update = update => send({ method: 'session/update', params: { sessionId: params.sessionId, update } });
      update({ sessionUpdate: 'agent_thought_chunk', content: { type: 'text', text: 'ACP recovery reasoning' } });
      update({ sessionUpdate: 'tool_call', toolCallId: 'recovery-read', title: 'Read sample.ts',
        kind: 'read', status: 'in_progress', rawInput: { file_path: 'sample.ts' }, content: [] });
      update({ sessionUpdate: 'tool_call_update', toolCallId: 'recovery-read', status: 'completed',
        content: [{ type: 'content', content: { type: 'text', text: 'Fixture read result' } }] });
    }

    item.timer=setInterval(()=>{item.ticks++;notify(params.sessionId,` ${token}:${item.ticks}`)},180);
    if(token.startsWith('auto-'))item.auto=setTimeout(()=>finish(token),250);
    if(token.startsWith('error-'))item.auto=setTimeout(()=>finish(token,'end_turn',true),250);
    if(token.startsWith('exit-'))item.auto=setTimeout(()=>{event('prompt_exit',{token});process.exit(7)},350);
    return;
  }
  if(method==='session/cancel'){
    const item=active.get(params.sessionId);event('cancel',{token:item?.token});
    if(item&&!item.token.includes('ignore-cancel'))setTimeout(()=>finish(item.token,'cancelled'),item.token.includes('delayed-cancel')?1600:30);
    return;
  }
  if(method==='session/set_mode'||method==='session/set_model'||method==='session/set_config_option'){send({id,result:{}});return;}
  if(method==='session/list'){send({id,result:{sessions:[]}});return;}
  if(id!==undefined)send({id,error:{code:-32601,message:'Method not implemented by controlled fixture: '+method}});
});
lines.on('close',()=>{for(const item of active.values())clearInterval(item.timer);control.close();process.exit(0)});
