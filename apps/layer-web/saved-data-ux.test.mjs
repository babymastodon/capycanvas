import assert from 'node:assert/strict';
import {mkdir,readFile,writeFile} from 'node:fs/promises';

export async function checkSavedDataUx({call,evaluate,reload,settle}) {
  const output=process.env.LAYER_TEST_ARTIFACTS||'artifacts/saved-data-compat/ux-shots';
  await mkdir(output,{recursive:true});
  const messages={
    newer:"Your workspaces were saved by a newer version of Capy Canvas. Update Capy Canvas to save changes to workspaces, palettes and brush settings.",
    unavailable:"Capy Canvas can't open your saved workspaces. Restart Capy Canvas to save changes to workspaces, palettes and brush settings.",
    damaged:"Your saved workspaces were damaged. Capy Canvas started new ones.",
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
  await ready();
  const baseline=await storage("const read=store.get('database');read.onsuccess=()=>{result=read.result.snapshot;};");
  const evidence=[];
  for(const theme of ['light','dark']) {
    for(const [kind,expected] of Object.entries(messages)) {
      await wait('!JSON.parse(layerApp.app.workspace_view()).dirty&&!JSON.parse(layerApp.app.workspace_view()).saving&&!layerApp.documents.busy()&&!document.querySelector("dialog[open]")');
      const database=JSON.parse(baseline);
      if(kind==='newer')database.schema++;
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
      if(kind.startsWith('drawing-')) {
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
      if(kind==='newer')assert.equal(JSON.parse(await storage("const read=store.get('database');read.onsuccess=()=>{result=read.result.snapshot;};")).schema,database.schema);
    }
  }
  const store1012=await readFile(new URL('../../crates/layer-workspace/tests/fixtures/web-store-1.0.12.json',import.meta.url),'utf8');
  const earlier=await readFile(new URL('../../crates/layer-workspace/tests/fixtures/web-store-1.0.15.json',import.meta.url),'utf8');
  const workspaces={sketch:'builtin:workspace:painter',paint:'builtin:workspace:illustrator',photo:'builtin:workspace:photographer'};
  const toolbars=layout=>Object.fromEntries(layout.panels.filter(panel=>panel.content.kind==='toolbar').map(panel=>[panel.id,panel.content.tiles
    .filter(tile=>!['opacity','brush_opacity_slider'].includes(tile.control.kind)&&tile.control.command!=='decoration').map(tile=>tile.id)]));
  const quiet=async step=>{
    const view=JSON.parse(await evaluate('layerApp.app.workspace_view()'));
    assert.equal(view.error,null,step);assert.equal(view.status,null,step);assert.equal(view.prompt,null,step);
    assert.equal(await evaluate("[...document.querySelectorAll('dialog[open]')].length"),0,`${step} asks nothing`);
  };
  for(const theme of ['light','dark']) {
    await wait('!JSON.parse(layerApp.app.workspace_view()).dirty&&!JSON.parse(layerApp.app.workspace_view()).saving');
    await storage(`store.clear();store.put({id:'database',snapshot:${JSON.stringify(store1012)}});`);
    await evaluate(`localStorage.setItem('layer.preferences.v1',JSON.stringify({...layerApp.state().settings,language:{Explicit:'en'},theme:${JSON.stringify(theme)}}))`);
    await restart();
    await ready();
    await quiet('startup');
    for(const [name,id] of Object.entries(workspaces)) {
      await evaluate(`layerApp.app.workspace_input(JSON.stringify({type:'switch',id:${JSON.stringify(id)}}));null`);
      await wait(`JSON.parse(layerApp.app.workspace_view()).id===${JSON.stringify(id)}&&JSON.parse(layerApp.app.workspace_view()).ready&&!JSON.parse(layerApp.app.workspace_view()).busy`);
      await quiet(name);
      const history=JSON.parse(store1012).items[id].entity.content.history;
      assert.deepEqual(toolbars(await evaluate('layerApp.state().workspace.layout')),toolbars(history.revisions[history.current].layout),`${name} keeps every released tile this build reads`);
      await shot(`web-released-${name}-${theme}`);
    }
    await wait('!JSON.parse(layerApp.app.workspace_view()).dirty&&!JSON.parse(layerApp.app.workspace_view()).saving');
    const stored=JSON.parse(await storage("const read=store.get('database');read.onsuccess=()=>{result=read.result.snapshot;};"));
    for(const id of Object.values(workspaces))assert.deepEqual(stored.items[id].entity.content,JSON.parse(store1012).items[id].entity.content,`opening ${id} rewrote none of it`);
    evidence.push({name:`web-released-${theme}`,source:'released 1.0.12 store as the current IndexedDB store'});
  }
  const restored="Your saved workspaces were damaged. Capy Canvas restored an earlier copy.";
  const records=()=>storage('const request=store.getAll();request.onsuccess=()=>{result=request.result.map(({id,snapshot})=>({id,snapshot}));};');
  for(const theme of ['light','dark']) {
    await wait('!JSON.parse(layerApp.app.workspace_view()).dirty&&!JSON.parse(layerApp.app.workspace_view()).saving');
    const older=`snapshot:workspaces.1.0.14.${crypto.randomUUID()}`,newest=`snapshot:workspaces.1.0.15.${crypto.randomUUID()}`,truncated=earlier.slice(0,earlier.length>>1);
    const copies=[{id:older,bytes:earlier.length},{id:newest,bytes:truncated.length}];
    await storage(`store.clear();store.put({id:'database',snapshot:${JSON.stringify(truncated)}});store.put({id:${JSON.stringify(older)},snapshot:${JSON.stringify(earlier)}});store.put({id:${JSON.stringify(newest)},snapshot:${JSON.stringify(truncated)}});store.put({id:'version',value:{version:'',unreadable_identity:null,copies:${JSON.stringify(copies)}}});`);
    await evaluate(`localStorage.setItem('layer.preferences.v1',JSON.stringify({...layerApp.state().settings,language:{Explicit:'en'},theme:${JSON.stringify(theme)}}))`);
    await restart();
    await ready();
    await wait(`layerApp.state().notice?.text===${JSON.stringify(restored)}`);
    await wait(`(()=>{const notice=document.querySelector('.canvas-notice');return notice&&!notice.hidden&&notice.textContent.includes(${JSON.stringify(restored)});})()`);
    assert.equal(await evaluate("document.querySelector('.canvas-notice').querySelectorAll('button:not([hidden])').length"),0,'The notice has no buttons');
    await quiet('restored');
    const name=`web-set-aside-${theme}`;
    await shot(name);
    const rows=await records();
    assert.deepEqual(rows.filter(row=>row.id.startsWith('damaged:')).map(row=>row.snapshot),[truncated],'The damaged store is kept aside');
    assert.ok(!rows.some(row=>row.id===older),'The installed copy is consumed');
    assert.ok(rows.some(row=>row.id===newest),'A copy that can\'t be read stays retained');
    assert.deepEqual(Object.keys(JSON.parse(rows.find(row=>row.id==='database').snapshot).items).sort(),Object.keys(JSON.parse(earlier).items).sort());
    for(const id of Object.values(workspaces)) {
      await evaluate(`layerApp.app.workspace_input(JSON.stringify({type:'switch',id:${JSON.stringify(id)}}));null`);
      await wait(`JSON.parse(layerApp.app.workspace_view()).id===${JSON.stringify(id)}&&JSON.parse(layerApp.app.workspace_view()).ready&&!JSON.parse(layerApp.app.workspace_view()).busy`);
      await quiet(`restored ${id}`);
    }
    await wait('!JSON.parse(layerApp.app.workspace_view()).dirty&&!JSON.parse(layerApp.app.workspace_view()).saving');
    await restart();await ready();
    assert.equal(await evaluate('layerApp.state().notice?.text??null'),null,'A second start shows no notice');
    await quiet('relaunch');
    assert.equal((await records()).filter(row=>row.id.startsWith('damaged:')).length,1,'A second start sets nothing aside');
    evidence.push({name,source:'1.0.15 IndexedDB store cut to half its length, with a readable older copy and an unreadable newer copy'});
  }
  await writeFile(`${output}/web-messages.json`,JSON.stringify(evidence,null,2));
  console.log('PASS saved-data messages: all approved messages, the released 1.0.12 workspaces and the damaged store replaced by its newest readable copy in both themes; real newer schema, truncated store and package readers');
}
