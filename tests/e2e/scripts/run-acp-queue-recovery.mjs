import assert from 'node:assert/strict';
import path from 'node:path';
import { readFile, writeFile, readdir } from 'node:fs/promises';
import { AcpQueueRecoveryPage } from '../page-objects/AcpQueueRecoveryPage.mjs';
import { installObserver } from '../helpers/acp-queue-observer.mjs';
import { openNativeWorkload, delay } from './native-workload-session.mjs';

const run = await openNativeWorkload({ Page: AcpQueueRecoveryPage,
  label: 'acp-queue-recovery', evidence: process.argv[2],
  harnessFiles: [
    'tests/e2e/scripts/run-acp-queue-recovery.mjs',
    'tests/e2e/page-objects/AcpQueueRecoveryPage.mjs',
    'tests/e2e/page-objects/SceneVisibilityPage.mjs',
    'tests/e2e/helpers/acp-queue-observer.mjs',
    'tests/e2e/fixtures/controlled-acp-agent.mjs',
  ],
  protocol: { name: 'acp-queue-recovery', version: 2, prewarmMs: 1000, functionalOnly: true },
});
const page = run.page;
const result = { passed: false, isolatedProfile: true, realAcpStdio: true,
  productStateInjected: false, native: run.report, stages: [] };
const save = () => writeFile(path.join(run.root, 'acp-queue-recovery.json'), JSON.stringify(result, null, 2));
const mark = (name, detail) => { result.stages.push({ at: Date.now(), name, ...detail }); console.log(name); };

// Discover the production bundle's existing exports. This observes the live
// singleton instances; it does not replace event handlers or write product state.
const escape = text => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
async function diagnosticsModules() {
  const assets = await readdir(path.join(run.root, 'frontend/assets'));
  const manager = assets.find(n => /^FlowChatManager-.*\.js$/.test(n));
  const state = assets.find(n => /^ReviewPlatformAPI-.*\.js$/.test(n));
  assert.ok(manager && state, 'Production singleton bundles are available');
  for (const name of assets.filter(n => /^index-.*\.js$/.test(n))) {
    const code = await readFile(path.join(run.root, 'frontend/assets', name), 'utf8');
    const gap = code.match(/function ([\w$]+)\([^)]*\)\{return [^{};]*\?\.hasGap\(\)===!0\}/)?.[1];
    const attach = code.match(/function ([\w$]+)\([^)]*\)\{return [^{};]*\?\.isReadInFlight\(\)===!0\}/)?.[1];
    if (!gap || !attach) continue;
    const notify = code.match(/\.markProjectionBehind\(\),([\w$]+)\(/)?.[1];
    assert.ok(notify, 'Gap notification function exists');
    const set = code.match(new RegExp('function ' + escape(notify) + '\\([^)]*\\)\\{for\\(const [\\w$]+ of ([\\w$]+)\\)'))?.[1];
    assert.ok(set, 'Gap observer set exists');
    const subscribe = code.match(new RegExp('function ([\\w$]+)\\([^)]*\\)\\{return ' + escape(set) + '\\.add\\('))?.[1];
    const exports = code.slice(code.lastIndexOf('export{'));
    const exported = local => {
      const key = exports.match(new RegExp('(?:[{,])' + escape(local) + ' as ([\\w$]+)'))?.[1];
      assert.ok(key, 'Diagnostic function is already exported: ' + local);
      return key;
    };
    return { manager: '/assets/' + manager, state: '/assets/' + state,
      gate: '/assets/' + name, gap: exported(gap), attach: exported(attach),
      subscribeGap: exported(subscribe) };
  }
  throw new Error('No compatible production SessionStream exports; rebuild-specific discovery needs updating');
}

let ids, modules;
const select = id => page.selectSession(id);
const modelReady = () => page.modelReady();
const send = token => page.sendCase(token);
async function events() {
  try { return (await readFile(path.join(run.root, 'acp-events.ndjson'), 'utf8')).trim().split('\n').map(JSON.parse); }
  catch (error) { if (error.code === 'ENOENT') return []; throw error; }
}
const seen = async (type, token) => (await events()).some(e => e.type === type && e.token === token);
const waitEvent = (type, token) => page.until(() => seen(type, token), type + ' ' + token, 6000);
async function control(token, action = 'finish') {
  const rows = await events();
  const prompt = rows.findLast(e => e.type === 'prompt' && e.token === token);
  const start = rows.findLast(e => e.type === 'started' && e.pid === prompt.pid);
  const response = await fetch('http://127.0.0.1:' + start.port, {
    method: 'POST', body: JSON.stringify({ token, action, reason: 'end_turn' }),
  });
  return response.json();
}
async function install() { await page.execute(installObserver, { ids, modules }); }
async function state(id) { return page.execute(id => window.__acpDiagnosticSnapshot?.().find(s => s.id === id), id); }
async function refresh() { await page.refreshDocument(); await install(); }
async function capture(name) {
  const value = await page.execute(() => ({ states: window.__acpDiagnosticSnapshot?.(),
    trace: window.__acpTrace, text: document.body.innerText.slice(-1000) }));
  await writeFile(path.join(run.root, name + '.json'), JSON.stringify(value, null, 2));
  return value;
}

try {
  modules = await diagnosticsModules();
  await page.click('[data-testid="nav-session-item"][data-session-id="workload-history-000"]');
  const workspaceId = await page.execute(async modulePath => {
    const m = await import(modulePath);
    const manager = Object.values(m).find(v => typeof v === 'function' && v.getInstance && v.prototype.sendMessage);
    return manager.getInstance().context.flowChatStore.getState().sessions.get('workload-history-000').workspaceId;
  }, modules.manager);
  await page.invoke('save_acp_json_config', { jsonConfig: JSON.stringify({ acpClients: { controlled: {
    name: 'Controlled ACP', command: process.execPath, args: [path.join(run.root, 'harness/tests/e2e/fixtures/controlled-acp-agent.mjs')],
    env: { OPENBITFUN_ACP_FIXTURE_LOG: path.join(run.root, 'acp-events.ndjson') }, enabled: true,
  } } }) });
  const create = async name => (await page.invoke('create_acp_flow_session', { request: {
    clientId: 'controlled', sessionName: name, workspaceId, workspacePath: run.workspace,
  } })).sessionId;
  const A = await create('ACP recovery A'), B = await create('ACP switch target B');
  ids = [A, B]; result.ids = { A, B }; result.modules = modules;
  await select(A); await modelReady(); await install();
  const saveCase = async (name, detail) => { mark(name, detail); await save(); };
  const promptCount = async token => (await events()).filter(e => e.type === 'prompt' && e.token === token).length;
  const snapshot = () => page.invoke('restore_session_view', { request: {sessionId:A,workspaceId,workspacePath:run.workspace,tailTurnCount:8} });
  // Metadata must remain readable while the same ACP prompt owns its receiver.
  await send('hold-metadata-reading'); await waitEvent('prompt', 'hold-metadata-reading');
  const assets = await readdir(path.join(run.root, 'frontend/assets'));
  const apiModule = '/assets/' + assets.find(name => /^ACPClientAPI-.*\.js$/.test(name));
  const request = { sessionId: A, clientId: 'controlled', workspaceId, workspacePath: run.workspace };
  const readMetadata = () => page.execute(async ({ request, apiModule }) => {
    const module = await import(apiModule);
    const api = Object.values(module).find(value => typeof value === 'function' && value.getSessionOptions);
    const start = performance.now();
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const reads = Promise.all([
      invoke('get_acp_session_options', { request }),
      invoke('get_acp_session_commands', { request }),
      api.getSessionOptions(request), api.getSessionCommands(request),
    ]);
    let timeout;
    try {
      const values = await Promise.race([reads, new Promise((_, reject) => {
        timeout = setTimeout(() => reject(new Error('ACP metadata blocked by a running prompt')), 2000);
      })]);
      return { elapsedMs: performance.now() - start, values };
    } finally { clearTimeout(timeout); }
  }, { request, apiModule });
  const metadataReads = [await readMetadata()];
  for (let index = 0; index < 3; index++) {
    await select(B); await modelReady(); await select(A); await modelReady();
    metadataReads.push(await readMetadata());
  }
  assert.ok(!(await seen('finished', 'hold-metadata-reading')));
  await control('hold-metadata-reading', 'metadata');
  let updated;
  await page.until(async () => {
    updated = await readMetadata();
    return updated.values[0].availableCommands.some(command => command.name === 'inspect')
      && updated.values[0].configOptions.some(option => option.id === 'reasoning' && option.currentValue === 'high')
      && updated.values[0].contextUsage?.used === 19;
  }, 'live metadata snapshot updates', 6000);
  assert.ok(updated.values[1].some(command => command.name === 'inspect'));
  await saveCase('metadata-reads-and-live-updates-survive-running-session-switches', { metadataReads, updated });

  // ACP send-now promotes queued input; it must not overlap the running prompt.
  await send('auto-acp-queued-first'); await send('auto-acp-promoted-second');
  await page.until(async () => (await state(A)).queue.flat().length === 2, 'two ACP queued messages', 6000);
  await page.promoteQueuedCase(1);
  await page.until(async () => (await state(A)).queue.flat()[0]?.content.includes('auto-acp-promoted-second'), 'ACP send-now promotion', 4000);
  await select(B); await select(A); await modelReady();
  assert.equal(await promptCount('auto-acp-promoted-second'), 0);
  assert.equal(await promptCount('auto-acp-queued-first'), 0);
  await control('hold-metadata-reading');
  await waitEvent('finished', 'auto-acp-promoted-second');
  await waitEvent('finished', 'auto-acp-queued-first');
  const ordered = (await events()).filter(event => event.type === 'prompt').map(event => event.token);
  assert.ok(ordered.indexOf('auto-acp-promoted-second') < ordered.indexOf('auto-acp-queued-first'));
  assert.equal(await promptCount('auto-acp-promoted-second'), 1);
  assert.equal(await promptCount('auto-acp-queued-first'), 1);
  await page.until(async () => (await state(A))?.state === 'idle', 'promoted ACP queue settles', 6000);
  await saveCase('acp-send-now-and-switch-preserve-order-and-no-overlap', { ordered });

  await page.invoke('set_acp_session_model', { request: { ...request, modelId: 'fixture-alt' } });
  await send('hold-model-snapshot'); await waitEvent('prompt', 'hold-model-snapshot');
  const changedModel = await readMetadata();
  assert.equal(changedModel.values[0].currentModelId, 'fixture-alt');
  await control('hold-model-snapshot'); await waitEvent('finished', 'hold-model-snapshot');
  await page.until(async () => (await state(A))?.state === 'idle', 'model snapshot turn settles', 6000);
  await saveCase('successful-model-change-is-visible-during-the-next-prompt', { changedModel });

  await send('hold-before-refresh'); await waitEvent('prompt', 'hold-before-refresh');
  await send('auto-restored-queue');
  await page.until(async () => (await state(A)).queue.flat().length === 1, 'first queue persisted', 6000);
  await control('hold-before-refresh','pause');
  await select(B); await refresh();
  // Do not activate A: its persisted queue must acquire its host's running
  // execution in the background, without submitting a second ACP prompt.
  await page.until(async () => (await state(A))?.state === 'processing' && !(await state(A))?.gap, 'background running execution recovered', 10000);
  assert.equal(await promptCount('auto-restored-queue'), 0);
  const live = await snapshot();
  assert.equal(live.executionProjection,'acp');
  assert.ok(live.session.state.startsWith('Processing'));
  assert.ok(live.runtimeEventSnapshot.events.some(e=>e.eventName==='agentic://dialog-turn-started'));
  assert.ok(live.runtimeEventSnapshot.events.some(e => e.eventName === 'agentic://tool-event'));
  assert.ok(live.runtimeEventSnapshot.events.some(e => e.eventName === 'agentic://text-chunk'
    && e.payload.contentType === 'thinking'));

  await control('hold-before-refresh','resume');
  await delay(500);
  assert.equal(await promptCount('auto-restored-queue'),0);
  await saveCase('background-reload-keeps-the-original-prompt-running', {state:await state(A),snapshot:live});
  await capture('active-recovery');
  await control('hold-before-refresh');
  await waitEvent('finished','auto-restored-queue');
  await page.until(async()=>(await state(A))?.state==='idle'&&!(await state(A))?.gap,'background queue drained',8000);
  assert.equal(await promptCount('auto-restored-queue'),1);
  await select(A);await modelReady();
  await page.until(async () => {
    const recovered = (await snapshot()).turns.find(turn => turn.turnId === live.runtimeEventSnapshot.activeTurnId);
    return recovered?.modelRounds.some(round => round.textItems.some(
      item => item.content.includes('ACP_STARTED:hold-before-refresh'),
    ));
  }, 'original ACP output persisted after replay', 6000);
  await saveCase('restored-queue-drained-once-without-another-refresh',{state:await state(A)});
  await page.shot(path.join(run.root,'recovered-queue.png'));
  await capture('after-recovery');

  // Original failure's second turn: completing normally must no longer leave
  // a stale presentation gate behind an otherwise idle ACP session.
  await send('hold-after-refresh');await waitEvent('prompt','hold-after-refresh');
  await send('auto-after-refresh');
  await page.until(async()=>(await state(A)).queue.flat().length===1,'second queue',6000);
  await select(B);await select(A);
  await control('hold-after-refresh');await waitEvent('finished','auto-after-refresh');
  await page.until(async()=>(await state(A))?.state==='idle'&&!(await state(A))?.gap,'second queue settles',8000);
  assert.equal(await promptCount('auto-after-refresh'),1);
  await saveCase('original-stall-sequence-now-drains-automatically',{state:await state(A)});

  await modelReady();
  await send('hold-delayed-cancel-acceptance');await waitEvent('prompt','hold-delayed-cancel-acceptance');
  await send('auto-after-cancel-acceptance');
  await page.until(async()=>(await state(A)).queue.flat().length===1,'cancel queue',6000);
  await page.click('[data-testid="chat-input-cancel-btn"]');
  await select(B);await select(A);await delay(400);
  assert.equal(await promptCount('auto-after-cancel-acceptance'),0,'cancel ACK must not release queue');
  await waitEvent('finished','auto-after-cancel-acceptance');
  await page.until(async()=>(await state(A))?.state==='idle'&&!(await state(A))?.cancelGate,'cancelled queue settles',8000);
  await saveCase('delayed-cancel-and-switch-drains-after-real-terminal',{state:await state(A)});

  await select(B);await modelReady();
  const direct={sessionId:B,clientId:'controlled',workspaceId,workspacePath:run.workspace,
    turnId:'acceptance-idempotent-turn',userInput:'ACP_CASE:hold-idempotent'};
  await page.invoke('start_acp_dialog_turn',{request:direct});
  await waitEvent('prompt','hold-idempotent');
  await page.invoke('start_acp_dialog_turn',{request:direct});
  await assert.rejects(()=>page.invoke('start_acp_dialog_turn',{request:{...direct,turnId:'overlap',userInput:'ACP_CASE:auto-overlap'}}),/still executing/);
  await assert.rejects(()=>page.invoke('start_acp_dialog_turn',{request:{...direct,userInput:'changed'}}),/different input/);
  assert.equal(await promptCount('hold-idempotent'),1);
  await control('hold-idempotent');
  await waitEvent('finished','hold-idempotent');
  await page.until(async()=>(await state(B))?.state==='idle','direct turn completed',8000);
  await page.invoke('start_acp_dialog_turn',{request:direct});
  assert.equal(await promptCount('hold-idempotent'),1,'terminal retry must not re-run prompt');
  await saveCase('duplicate-and-overlapping-submission-admission-correct',{});

  await send('hold-error-acceptance');await waitEvent('prompt','hold-error-acceptance');
  await send('auto-after-error-acceptance');
  await page.until(async()=>(await state(B)).queue.flat().length===1,'error queue',6000);
  await control('hold-error-acceptance','error');
  await waitEvent('finished','auto-after-error-acceptance');
  await page.until(async()=>(await state(B))?.state==='idle','error recovery idle',8000);
  await saveCase('protocol-error-drains-follow-up',{});

  await modelReady();
  await send('hold-exit-acceptance');await waitEvent('prompt','hold-exit-acceptance');
  await send('auto-after-exit-acceptance');
  await page.until(async()=>(await state(B)).queue.flat().length===1,'exit queue',6000);
  await control('hold-exit-acceptance','exit');
  await waitEvent('finished','auto-after-exit-acceptance');
  await page.until(async()=>(await state(B))?.state==='idle'&&!(await state(B))?.gap,'restarted ACP settles',8000);
  await saveCase('agent-process-exit-reconnects-and-drains',{});
  await capture('final-state');
  result.passed = true;
  run.report.functionalPassed = true;
  run.report.passed = true;
  await save();
  console.log(JSON.stringify({passed:true,root:run.root,frontend:run.report.frontend.revision,cases:result.stages.map(s=>s.name)}));

} catch (error) {
  result.error = error.stack || String(error);
  await capture('failure').catch(() => {});
  await page.shot(path.join(run.root, 'failure.png')).catch(() => {});
  await save();
  throw error;
} finally {
  await run.close();
}
