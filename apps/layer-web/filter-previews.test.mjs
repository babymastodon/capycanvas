import {histogramJourney} from './histogram-journey.mjs';
import assert from "node:assert/strict";
import {mkdir,writeFile} from "node:fs/promises";

// Real Wasm/GPU and native presentation; observe the existing bridge without
// adding production instrumentation or driving a second preview producer.
export async function checkFilterPreviews({call,evaluate,settle}) {
  const wait=condition=>evaluate(`new Promise((resolve,reject)=>{
    const end=performance.now()+45000;function check(){
      if(${condition})resolve(true);
      else if(performance.now()>end)reject(Error('Preview timeout: '+${JSON.stringify(condition)}));
      else setTimeout(check,40);
    }check();})`);
  const histogram=histogramJourney({evaluate,settle}).exact;
  const directory=process.env.LAYER_TEST_ARTIFACTS||'artifacts/filter-memory';
  const timing={};
  const timed=async(name,action,condition)=>{
    const started=performance.now();await action();await wait(condition);
    timing[name]=performance.now()-started;
  };
  await mkdir(directory,{recursive:true});
  const capture=async name=>{
    const shot=await call('Page.captureScreenshot',{format:'png'});
    await writeFile(`${directory}/${name}.png`,Buffer.from(shot.data,'base64'));
  };
  const blank=await histogram();
  await evaluate(`(async()=>{
    const canvas=new OffscreenCanvas(256,256),context=canvas.getContext('2d',{willReadFrequently:true});
    for(let y=0;y<8;y++)for(let x=0;x<8;x++){context.fillStyle=(x+y)%2?'rgb(30,160,220)':'rgb(230,50,80)';context.fillRect(x*32,y*32,32,32);}
    const pixel=Array.from(context.getImageData(0,0,1,1).data);
    if(pixel.join(',')!=='230,50,80,255')throw Error('Invalid generated checker pixel: '+pixel);
    const file=new File([await canvas.convertToBlob()],'Preview checker.png',{type:'image/png'});
    window.previewPicker=window.showOpenFilePicker;window.showOpenFilePicker=async()=>[{getFile:async()=>file}];
    layerApp.dispatch({type:'invoke',command:'import_image'});
  })()`);
  await wait(`layerApp.state().commands.find(c=>c.id==='placement_original_size').enabled`);
  assert.equal(await evaluate('layerApp.documents.busy()'),false,'Image import host work has finished before Apply');
  assert.equal(await evaluate("layerApp.state().commands.find(command=>command.id==='apply_transform').enabled"),true,'The first imported placement can be applied');
  await evaluate(`window.showOpenFilePicker=previewPicker;delete window.previewPicker;layerApp.dispatch({type:'invoke',command:'apply_transform'})`);
  const applied=await evaluate("({busy:layerApp.documents.busy(),apply:layerApp.state().commands.find(command=>command.id==='apply_transform').enabled,tool:layerApp.state().layer_tools.tool,host_error:layerApp.state().host_error})");
  assert.equal(applied.apply,false,'The first Apply commits placement: '+JSON.stringify(applied));
  await wait(`layerApp.state().layer_tools.editing_layer.object&&layerApp.state().layer_tools.editing_layer.label==='Preview checker'&&layerApp.state().canvas_bar?.context.kind==='transform'&&!layerApp.state().canvas_bar.completion.length`);
  await wait('layerApp.app.brush_ready()');
  await capture('web-preview-import');
  const imported=await histogram();
  assert.notDeepEqual(imported,blank,'The imported checker changes document pixels');
  await evaluate(`(()=>{
    const app=layerApp.app,original=app.poll_filter_previews;
    window.previewCheck={restore(){app.poll_filter_previews=original;}};
    document.body.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,pointerId:987,pointerType:'pen'}));
    app.poll_filter_previews=function(...args){
      const result=original.apply(this,args);
      previewCheck.status=result;previewCheck.ids=args[0];return result;
    };
    previewCheck.image=id=>[...document.querySelectorAll('[data-effect="'+id+'"] img')].find(i=>i.getBoundingClientRect().height>0);
    previewCheck.pixels=id=>{const pixels=(i=>{if(!i?.complete||!i.naturalWidth)return false;const c=new OffscreenCanvas(i.naturalWidth,i.naturalHeight),d=c.getContext('2d');d.drawImage(i,0,0);return d.getImageData(0,0,c.width,c.height).data;})(previewCheck.image(id));return !!pixels&&pixels.some((v,i)=>i%4===3&&v>0);};
    layerApp.dispatch({type:'filter_picker',action:{op:'category',category:null}});
  })()`);
  const show=async()=>{
    await evaluate(`(()=>{if(![...document.querySelectorAll('.filter-picker')].some(p=>p.getBoundingClientRect().height>0))document.querySelector('.dock-tab[data-panel=adjustments]').click();})()`);
    await settle();
  };
  const status=()=>evaluate(`(()=>{const {atlas,bytes,...status}=previewCheck.status;return JSON.parse(JSON.stringify(status,(_,v)=>typeof v==='bigint'?Number(v):v));})()`);
  try {
    await settle();await show();
    await wait(`previewCheck.status&&!previewCheck.status.pending&&previewCheck.pixels('curves')`);
    assert.equal(await evaluate(`!!previewCheck.image('curves')&&!previewCheck.image('curves').parentElement.querySelector('canvas')`),true,'Held contact shows the loaded bundled image without a GPU preview canvas');
    assert.ok((await evaluate('previewCheck.ids')).includes('curves'),'Visible bundled rows reach shared preparation during a held contact');
    assert.equal((await status()).requests,0,'Held contact loads bundled previews without GPU requests');
    await timed('release_ms',()=>evaluate("document.body.dispatchEvent(new PointerEvent('pointerup',{bubbles:true,pointerId:987,pointerType:'pen'}));undefined"),
      `previewCheck.status&&!previewCheck.status.pending&&previewCheck.pixels('curves')`);
    await settle();
    assert.equal(await evaluate(`!!previewCheck.image('curves')&&!previewCheck.image('curves').parentElement.querySelector('canvas')`),true,'Release retains the loaded bundled image without a GPU preview canvas');
    assert.ok((await evaluate('previewCheck.ids')).includes('curves'),'Built-in rows reach shared preparation while showing their bundled previews');
    const first=await status();assert.equal(first.error??null,null);assert.equal(first.requests,0);
    for(const theme of ['light','dark']){
      await evaluate(`(()=>{
        layerApp.dispatch({type:'set_theme',theme:${JSON.stringify(theme)}});
        const row=document.querySelector('[data-effect="curves"]');
        previewCheck.row=row;previewCheck.preview=row.querySelector('img');previewCheck.label=row.textContent;
        const saved=JSON.stringify({...layerApp.state().settings,language:{Explicit:'ja'}});
        layerApp.dispatch({type:'restore_saved_settings',saved});
      })()`);
      await wait(`!layerApp.app.language_pending()&&document.documentElement.lang==='ja'`);
      assert.ok(await evaluate(`(()=>{
        const row=document.querySelector('[data-effect="curves"]');
        return row===previewCheck.row&&row.querySelector('img')===previewCheck.preview&&row.textContent!==previewCheck.label&&previewCheck.pixels('curves');
      })()`),'Language publication retains filter rows, canvases and preview pixels');
      const localized=await status();assert.equal(localized.key,first.key);assert.equal(localized.requests,first.requests,'Language publication reuses the preview atlas');
      await capture(`web-preview-language-${theme}`);
      await evaluate(`(()=>{const saved=JSON.stringify({...layerApp.state().settings,language:{Explicit:'en'}});layerApp.dispatch({type:'restore_saved_settings',saved});})()`);
      await wait(`!layerApp.app.language_pending()&&document.documentElement.lang==='en'`);
    }
    await evaluate(`document.querySelector('.dock-tab[data-panel=properties]').click()`);await settle();
    await wait(`previewCheck.ids.length===0&&!previewCheck.status.pending`);
    await show();
    await wait(`previewCheck.pixels('curves')`);
    const reopened=await status();
    assert.equal(reopened.key,first.key);assert.equal(reopened.requests,first.requests,"Reopening reuses the atlas");
    await evaluate(`layerApp.dispatch({type:'set_layer_opacity',opacity:.63})`);await settle();
    await wait(`!previewCheck.status.pending&&previewCheck.pixels('curves')`);
    const edited=await status();assert.equal(edited.key,first.key,'Document edits keep the preview rows');
    assert.equal(edited.requests,first.requests,'Previews show a fixed scene, so document edits never re-render them');assert.equal(edited.error??null,null);
    await timed('category_ms',()=>evaluate(`layerApp.dispatch({type:'filter_picker',action:{op:'category',category:'color'}})`),
      `previewCheck.pixels('hue_saturation')&&!previewCheck.status.pending`);
    const category=await status();assert.ok(category.retained.length<=64);assert.equal(category.error??null,null);
    const beforeRestart=await histogram();
    await capture('web-preview-before-restart');
    const restartStarted=performance.now();await evaluate(`layerApp.restartGpu()`);
    await wait('layerApp.app.brush_ready()&&layerApp.startupTimes.complete!==null');
    await wait(`previewCheck.status.key!==${JSON.stringify(category.key)}&&previewCheck.pixels('hue_saturation')&&!previewCheck.status.pending`);
    timing.restart_ms=performance.now()-restartStarted;
    const restarted=await status();assert.equal(restarted.error??null,null);
    const afterRestart=await histogram();
    await capture('web-preview-lifecycle');
    assert.deepEqual(afterRestart,beforeRestart,'GPU replacement preserves imported artwork');
    await writeFile(`${directory}/web-preview-lifecycle.json`,JSON.stringify({first,reopened,edited,category,restarted,timing},null,2));
    console.log(`Filter preview timings: ${JSON.stringify(timing)}`);
    console.log('PASS: bundled preview rows, hide/reopen reuse, document edits, category changes and GPU replacement');
  } finally { await evaluate(`document.body.dispatchEvent(new PointerEvent('pointercancel',{bubbles:true,pointerId:987,pointerType:'pen'}));previewCheck.restore();delete window.previewCheck`); }
}
