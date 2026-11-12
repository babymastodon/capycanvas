import assert from 'node:assert/strict';
import {mkdir,readFile,writeFile} from 'node:fs/promises';

export async function checkSavedDataUx({call,evaluate,reload,settle}) {
  const output=process.env.LAYER_TEST_ARTIFACTS||'artifacts/saved-data-compat/ux-shots';
  await mkdir(output,{recursive:true});
  const messages={
    newer:"Your workspaces were saved by a newer version of Capy Canvas. Update Capy Canvas to save changes to workspaces, palettes and brush settings.",
    unavailable:"Capy Canvas can't open your saved workspaces. Restart Capy Canvas to save changes to workspaces, palettes and brush settings.",
    damaged:"Your saved workspaces were damaged. Capy Canvas started new ones.",
    editing:"Some palettes or brush settings can't be opened in this version of Capy Canvas. Update Capy Canvas to open them.",
    workspace:"This version of Capy Canvas can't open your saved workspaces. Update Capy Canvas to open them.",
    choice:"This version of Capy Canvas can't open this workspace.",
    'drawing-unsupported':"It has layers or settings that this version of Capy Canvas can't edit.",
    'drawing-damaged':"Part of this file is damaged.",
  };
  const wait=condition=>evaluate(`new Promise((resolve,reject)=>{const end=performance.now()+120000;function poll(){if(${condition})resolve(true);else if(performance.now()>end)reject(Error(${JSON.stringify(condition)}));else setTimeout(poll,20);}poll();})`);
  const ready=async()=>{await wait('window.layerApp?.app.brush_ready()&&layerApp.app.document_park_ready()&&JSON.parse(layerApp.app.workspace_view()).ready&&!JSON.parse(layerApp.app.workspace_view()).busy&&!JSON.parse(layerApp.app.workspace_view()).dirty&&!JSON.parse(layerApp.app.workspace_view()).saving&&!JSON.parse(layerApp.app.workspace_view()).switcher_busy&&!layerApp.documents.busy()');await evaluate('layerApp.documents.startRecovery()');};
  const restart=async()=>{
    await evaluate('layerApp.documents.startRecovery().then(()=>layerApp.documents.autosave(true))');
    for(const type of ['mousePressed','mouseReleased'])await call('Input.dispatchMouseEvent',{type,x:1,y:1,button:'left',buttons:type==='mousePressed'?1:0,clickCount:1});
    await reload();
  };
  const storage=operation=>evaluate(`(async()=>{const db=await new Promise((resolve,reject)=>{const request=indexedDB.open('capycanvas.workspaces',1);request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error)});try{return await new Promise((resolve,reject)=>{const tx=db.transaction('workspace','readwrite'),store=tx.objectStore('workspace');let result;${operation}tx.oncomplete=()=>resolve(result);tx.onabort=()=>reject(tx.error);});}finally{db.close();}})()`);
  const shot=async name=>{
    await settle();
    const {data}=await call('Page.captureScreenshot',{format:'png'});
    await writeFile(`${output}/${name}.png`,Buffer.from(data,'base64'));
  };
  const template=JSON.parse(await readFile(new URL('../../crates/layer-workspace/tests/compat/1.0.15/editing.json',import.meta.url),'utf8'));
  await ready();
  const baseline=await storage("const read=store.get('database');read.onsuccess=()=>{result=read.result.snapshot;};");
  const evidence=[];
  for(const theme of ['light','dark']) {
    for(const [kind,expected] of Object.entries(messages)) {
      await wait('!JSON.parse(layerApp.app.workspace_view()).dirty&&!JSON.parse(layerApp.app.workspace_view()).saving&&!layerApp.documents.busy()&&!document.querySelector("dialog[open]")');
      const database=JSON.parse(baseline);
      if(kind==='newer')database.schema++;
      if(kind==='editing'){const editing=structuredClone(template);editing.preset='unreadable';database.editing=JSON.stringify(editing);}
      if(kind==='workspace')for(const record of Object.values(database.items)){if(record.entity.metadata.kind==='workspace'){record.entity.content={};record.claim=null;}}
      if(kind==='choice'){database.items['builtin:workspace:painter'].entity.content={};database.items['builtin:workspace:painter'].claim=null;}
      const stored=JSON.stringify(database);
      await storage(`store.clear();store.put({id:'database',snapshot:${JSON.stringify(kind==='damaged'?stored.slice(0,stored.length>>1):stored)}});`);
      await evaluate(`localStorage.setItem('layer.preferences.v1',JSON.stringify({...layerApp.state().settings,language:{Explicit:'en'},theme:${JSON.stringify(theme)}}))`);
      let injected;
      if(kind==='unavailable') {
        const source=`(()=>{const Base=Worker;window.Worker=class extends Base{constructor(url,options){super(url,options);this.savedDataStorage=options?.name==='workspace-storage';this.savedDataFailed=false;}postMessage(data,transfer){if(this.savedDataStorage&&data.request){const request=JSON.parse(data.request);if(request.type==='reopen'&&!this.savedDataFailed){this.savedDataFailed=true;queueMicrotask(()=>this.dispatchEvent(new MessageEvent('message',{data:{id:data.id,error:JSON.stringify({kind:'unavailable',message:'PRIVATE reader details'})}})));return;}}super.postMessage(data,transfer);}}})()`;
        injected=(await call('Page.addScriptToEvaluateOnNewDocument',{source})).identifier;
      }
      await restart();
      await ready();
      if(injected)await call('Page.removeScriptToEvaluateOnNewDocument',{identifier:injected});
      if(kind==='choice') {
        await evaluate(`layerApp.app.workspace_input(JSON.stringify({type:'open',page:'workspaces'}));layerApp.app.workspace_input(JSON.stringify({type:'select',id:'builtin:workspace:painter'}));null`);
        await wait(`document.querySelector('.workspace-manager[open]')?.textContent.includes(${JSON.stringify(expected)})`);
      } else if(kind.startsWith('drawing-')) {
        const fixture=await readFile(new URL(`../../crates/layer-core/tests/compat/1.0.15/${kind==='drawing-unsupported'?'unsupported-pass-through':'samples-U8'}.capy`,import.meta.url));
        const bytes=kind==='drawing-damaged'?fixture.subarray(0,Math.floor(fixture.length/2)):fixture;
        await wait('layerApp.app.brush_ready()&&!layerApp.documents.busy()');
        await evaluate(`(()=>{const bytes=Uint8Array.from(atob(${JSON.stringify(bytes.toString('base64'))}),c=>c.charCodeAt(0));window.savedDataOpening=layerApp.documents.openFiles([{file:new File([bytes],'fixture.capy')}]);return true;})()`);
        await wait(`document.querySelector('dialog[open].document-dialog')?.textContent.includes(${JSON.stringify(expected)})`);
      } else {
        await wait(`layerApp.state().notice?.text===${JSON.stringify(expected)}`);
        await wait(`(()=>{const notice=document.querySelector('.canvas-notice');return notice&&!notice.hidden&&notice.textContent.includes(${JSON.stringify(expected)});})()`);
        assert.equal(await evaluate('document.querySelector(".canvas-notice").textContent.includes("PRIVATE")'),false);
      }
      const name=`web-${kind}-${theme}`;
      await shot(name);
      evidence.push({name,expected,source:kind==='damaged'?'truncated IndexedDB store set aside':'shared startup or file reader'});
      if(kind.startsWith('drawing-')) {
        await evaluate(`[...document.querySelectorAll('dialog[open].document-dialog footer button')].find(button=>button.textContent==='Close').click()`);
        await evaluate('savedDataOpening');
      }
      if(kind==='choice')await evaluate('layerApp.app.workspace_input(JSON.stringify({type:"cancel"}));null');
      if(kind==='newer')assert.equal(JSON.parse(await storage("const read=store.get('database');read.onsuccess=()=>{result=read.result.snapshot;};")).schema,database.schema);
    }
  }
  const released=await readFile(new URL('../../crates/layer-workspace/tests/compat/1.0.15/browser.json',import.meta.url),'utf8');
  let reset=0;
  for(const theme of ['light','dark'])for(const restore of [false,true]) {
    await wait('!JSON.parse(layerApp.app.workspace_view()).dirty&&!JSON.parse(layerApp.app.workspace_view()).saving');
    const current=JSON.parse(baseline),editing=structuredClone(template);
    for(const values of Object.values(editing.tools.overrides))values.size=43;
    current.editing=JSON.stringify(editing);
    const legacy=released+'\n'.repeat(++reset);
    await storage(`store.put({id:'database',snapshot:${JSON.stringify(JSON.stringify(current))}});store.put({id:'unreadable',snapshot:${JSON.stringify(legacy)}});`);
    await evaluate(`localStorage.setItem('layer.preferences.v1',JSON.stringify({...layerApp.state().settings,language:{Explicit:'en'},theme:${JSON.stringify(theme)}}))`);
    await restart();
    await wait("document.querySelector('dialog.workspace-form[open]')?.textContent.includes('Keep Current')");
    const prompt=await evaluate('JSON.parse(layerApp.app.workspace_view()).prompt');
    assert.equal(prompt.title,'Restore Workspaces?');
    assert.equal(prompt.message,"An older version of Capy Canvas reset your workspaces, palettes and brush settings. Restoring replaces changes you made after the reset.");
    assert.equal(prompt.confirm,'Restore');assert.equal(prompt.cancel,'Keep Current');
    const before=await storage('const request=store.getAll();request.onsuccess=()=>{result=request.result.filter(row=>row.id.startsWith("snapshot:")).map(({id,snapshot})=>({id,snapshot}));};');
    await shot(`web-restore-${restore?'restore':'keep'}-${theme}`);
    await wait('!JSON.parse(layerApp.app.workspace_view()).switcher_busy');
    await evaluate(`[...document.querySelectorAll('dialog.workspace-form[open] footer button')].find(button=>button.textContent===${JSON.stringify(restore?'Restore':'Keep Current')}).click()`);
    await wait(`JSON.parse(layerApp.app.workspace_view()).prompt==null&&JSON.parse(layerApp.app.workspace_view()).ready&&layerApp.state().brush.diameter===${restore?37:43}`);
    const after=await storage('const request=store.getAll();request.onsuccess=()=>{result=request.result.filter(row=>row.id.startsWith("snapshot:")).map(({id,snapshot})=>({id,snapshot}));};');
    const added=after.filter(row=>!before.some(old=>old.id===row.id));
    assert.equal(added.length,restore?1:0);
    if(restore)assert.equal(JSON.parse(JSON.parse(added[0].snapshot).editing).tools.overrides['2'].size,43);
    await wait('!JSON.parse(layerApp.app.workspace_view()).dirty&&!JSON.parse(layerApp.app.workspace_view()).saving');
    await restart();await ready();
    assert.equal(await evaluate('JSON.parse(layerApp.app.workspace_view()).prompt'),null,'A second start does not ask again');
    assert.equal(await evaluate('layerApp.state().brush.diameter'),restore?37:43);
    evidence.push({name:`web-restore-${restore?'restore':'keep'}-${theme}`,source:'real IndexedDB legacy copy, approved prompt and persistent choice'});
  }
  const paint='builtin:workspace:illustrator';
  const unopenable=await readFile(new URL('../../crates/layer-workspace/tests/fixtures/web-store-1.0.12.json',import.meta.url),'utf8');
  const earlier=await readFile(new URL('../../crates/layer-workspace/tests/fixtures/web-store-1.0.15.json',import.meta.url),'utf8');
  for(const theme of ['light','dark'])for(const retained of [false,true]) {
    await wait('!JSON.parse(layerApp.app.workspace_view()).dirty&&!JSON.parse(layerApp.app.workspace_view()).saving');
    const copy=`snapshot:workspaces.1.0.15.${crypto.randomUUID()}`;
    const copies=retained?`store.put({id:${JSON.stringify(copy)},snapshot:${JSON.stringify(earlier)}});store.put({id:'version',value:{version:'',unreadable_identity:null,copies:[{id:${JSON.stringify(copy)},bytes:${earlier.length}}]}});`:'';
    await storage(`store.clear();store.put({id:'database',snapshot:${JSON.stringify(unopenable)}});${copies}`);
    await evaluate(`localStorage.setItem('layer.preferences.v1',JSON.stringify({...layerApp.state().settings,language:{Explicit:'en'},theme:${JSON.stringify(theme)}}))`);
    await restart();
    await ready();
    assert.notEqual(await evaluate('JSON.parse(layerApp.app.workspace_view()).id'),paint,'Startup opens a workspace this version can read');
    await evaluate(`layerApp.app.workspace_input(JSON.stringify({type:'switch',id:${JSON.stringify(paint)}}));null`);
    const label=retained?'Restore Earlier Workspaces…':'Restore Starting Layout…';
    await wait(`(()=>{const status=document.querySelector('.workspace-status');return status&&!status.hidden&&status.textContent.includes(${JSON.stringify(messages.choice)})&&status.querySelector('.workspace-recovery')?.textContent===${JSON.stringify(label)}&&!status.querySelector('.workspace-recovery').hidden;})()`);
    assert.equal(await evaluate("[...document.querySelectorAll('.workspace-status button')].filter(button=>!button.hidden).length"),1,'Only the recovery is offered');
    const name=`web-recover-${retained?'earlier':'starting'}-${theme}`;
    await shot(`${name}-status`);
    await evaluate("document.querySelector('.workspace-status .workspace-recovery').click()");
    await wait("document.querySelector('dialog.workspace-form[open]')?.textContent.includes('Restore')");
    const prompt=await evaluate('JSON.parse(layerApp.app.workspace_view()).prompt');
    assert.equal(prompt.title,retained?'Restore Earlier Workspaces?':'Restore Starting Layout');
    assert.equal(prompt.message,retained?'Your workspaces, palettes and brush settings return to an earlier copy. The current ones are kept as a copy.':'Paint returns to its starting layout. A copy of your current workspaces is kept.');
    await shot(`${name}-prompt`);
    const before=await storage('const request=store.getAllKeys();request.onsuccess=()=>{result=request.result.filter(id=>id.startsWith("snapshot:"));};');
    await evaluate("[...document.querySelectorAll('dialog.workspace-form[open] footer button')].find(button=>button.textContent==='Restore').click()");
    await wait(`JSON.parse(layerApp.app.workspace_view()).ready&&JSON.parse(layerApp.app.workspace_view()).id===${JSON.stringify(paint)}&&JSON.parse(layerApp.app.workspace_view()).prompt==null&&!JSON.parse(layerApp.app.workspace_view()).status`);
    const after=await storage('const request=store.getAll();request.onsuccess=()=>{result=request.result.filter(row=>row.id.startsWith("snapshot:")).map(({id,snapshot})=>({id,snapshot}));};');
    const added=after.filter(row=>!before.includes(row.id));
    assert.equal(added.length,1,'The workspaces this version could not open are kept as a copy');
    assert.equal(JSON.stringify(JSON.parse(added[0].snapshot).items[paint].entity),JSON.stringify(JSON.parse(unopenable).items[paint].entity));
    evidence.push({name,source:'released 1.0.12 store as the current IndexedDB store'+(retained?' with a retained 1.0.15 copy':'')});
  }
  for(const theme of ['light','dark']) {
    await wait('!JSON.parse(layerApp.app.workspace_view()).dirty&&!JSON.parse(layerApp.app.workspace_view()).saving');
    const copy=`snapshot:workspaces.1.0.15.${crypto.randomUUID()}`,truncated=earlier.slice(0,earlier.length>>1);
    await storage(`store.clear();store.put({id:'database',snapshot:${JSON.stringify(truncated)}});store.put({id:${JSON.stringify(copy)},snapshot:${JSON.stringify(earlier)}});store.put({id:'version',value:{version:'',unreadable_identity:null,copies:[{id:${JSON.stringify(copy)},bytes:${earlier.length}}]}});`);
    await evaluate(`localStorage.setItem('layer.preferences.v1',JSON.stringify({...layerApp.state().settings,language:{Explicit:'en'},theme:${JSON.stringify(theme)}}))`);
    await restart();
    await wait("document.querySelector('dialog.workspace-form[open]')?.textContent.includes('Restore')");
    await wait(`layerApp.state().notice?.text===${JSON.stringify(messages.damaged)}`);
    const prompt=await evaluate('JSON.parse(layerApp.app.workspace_view()).prompt');
    assert.equal(prompt.title,'Restore Earlier Workspaces?');
    assert.deepEqual(await evaluate('JSON.parse(layerApp.app.workspace_view()).prompt_action'),{type:'restore_copy',value:copy});
    const name=`web-set-aside-${theme}`;
    await shot(name);
    const damaged=await storage('const request=store.getAll();request.onsuccess=()=>{result=request.result.filter(row=>row.id.startsWith("damaged:")).map(row=>row.snapshot);};');
    assert.deepEqual(damaged,[truncated],'The damaged store is kept aside');
    await evaluate("[...document.querySelectorAll('dialog.workspace-form[open] footer button')].find(button=>button.textContent==='Restore').click()");
    await wait("JSON.parse(layerApp.app.workspace_view()).ready&&JSON.parse(layerApp.app.workspace_view()).prompt==null&&!JSON.parse(layerApp.app.workspace_view()).status");
    await wait('!JSON.parse(layerApp.app.workspace_view()).dirty&&!JSON.parse(layerApp.app.workspace_view()).saving');
    await restart();await ready();
    assert.equal(await evaluate('JSON.parse(layerApp.app.workspace_view()).prompt'),null,'A second start does not ask again');
    assert.equal(await evaluate('layerApp.state().notice?.text??null'),null,'A second start shows no damaged notice');
    assert.equal(await evaluate("JSON.parse(layerApp.app.workspace_view()).id"),'builtin:workspace:illustrator');
    evidence.push({name,source:'1.0.15 IndexedDB store cut to half its length, with a retained 1.0.15 copy'});
  }
  await writeFile(`${output}/web-messages.json`,JSON.stringify(evidence,null,2));
  console.log('PASS saved-data messages: all approved messages, both restore choices, both workspace recoveries and the damaged-store set-aside rendered in both themes; real newer schema, truncated store, discarded editing, unreadable workspaces and package readers');
}
