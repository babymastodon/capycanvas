import {writeDrawerAudit} from './phone-audit.mjs';
import assert from 'node:assert/strict';
import {mkdir, writeFile} from 'node:fs/promises';

export async function checkBrushDrawers({call, evaluate, settle}) {
  const pause = () => new Promise(resolve => setTimeout(resolve, 220));
  const send = async action => {
    if(action.type==='workspace_manager') await evaluate(`layerApp.app.workspace_input(${JSON.stringify(JSON.stringify(action.command))});null`);
    else await evaluate(`layerApp.dispatch(${JSON.stringify(action)})`);
    await settle(); await pause();
  };
  const wait = expression => evaluate(`new Promise((resolve,reject)=>{const end=performance.now()+30000;function check(){if(${expression})resolve(true);else if(performance.now()>end)reject(Error(${JSON.stringify(expression)}));else setTimeout(check,50);}check();})`);
  const original = await evaluate('JSON.parse(layerApp.app.workspace_view()).id');
  await send({type:'workspace_manager',command:{type:'switch',id:'builtin:workspace:painter'}});
  await wait('JSON.parse(layerApp.app.workspace_view()).id==="builtin:workspace:painter" && !JSON.parse(layerApp.app.workspace_view()).busy');
  await send({type:'workspace_manager',command:{type:'form',action:{type:'new'}}});
  await send({type:'workspace_manager',command:{type:'submit',name:`Brush drawer test ${Date.now()}`}});
  await wait('!JSON.parse(layerApp.app.workspace_view()).busy && !JSON.parse(layerApp.app.workspace_view()).dirty');
  const created = await evaluate('JSON.parse(layerApp.app.workspace_view()).id');
  assert.notEqual(created,'builtin:workspace:painter');
  const theme = await evaluate('layerApp.state().settings.theme ?? null');
  const headers = await evaluate('layerApp.state().workspace.layout.header.zones.flat()');
  const header = command => `[data-header-item="${headers.find(e=>e.item.control?.command===command).id}"] .header-tool`;
  const drawer = () => evaluate('layerApp.state().customization.drawer');
  const contact = async (selector, device='mouse') => {
    const p = await evaluate(`(()=>{const n=document.querySelector(${JSON.stringify(selector)});n.scrollIntoView({block:'nearest'});const r=n.getBoundingClientRect();if(!r.width||!r.height)throw Error('Hidden '+${JSON.stringify(selector)});return{x:r.x+r.width/2,y:r.y+r.height/2}})()`);
    if(device==='touch') {
      await call('Input.dispatchTouchEvent',{type:'touchStart',touchPoints:[{id:1,...p}]});
      await call('Input.dispatchTouchEvent',{type:'touchEnd',touchPoints:[]});
    } else {
      await call('Input.dispatchMouseEvent',{type:'mouseMoved',...p,buttons:0,pointerType:device});
      await call('Input.dispatchMouseEvent',{type:'mousePressed',...p,button:'left',buttons:1,clickCount:1,pointerType:device,force:.7});
      await call('Input.dispatchMouseEvent',{type:'mouseReleased',...p,button:'left',buttons:0,clickCount:1,pointerType:device});
    }
    await settle(); await pause();
  };
  const set = (panel,label) => `.content-drawer [data-control="${panel}"] [data-tool-choice="${label}"]`;
  const check = async first => {
    assert.deepEqual((await drawer()).columns,[[first],['tools'],['tool_settings']]);
    assert.equal(await evaluate('document.querySelectorAll(".content-drawer > .drawer-contents > .drawer-column").length'),3);
    const widths = await evaluate('[...document.querySelectorAll(".content-drawer > .drawer-contents > .drawer-column")].map(n=>n.getBoundingClientRect().width)');
    assert.ok(widths[0]<widths[1] && widths[1]===widths[2],`Narrow sets, equal tools and settings: ${widths}`);
    assert.equal(await evaluate('document.querySelectorAll(".content-drawer .tools-control .tool-groups").length'),0);
    assert.ok(await evaluate('document.querySelectorAll(".content-drawer [data-tool-setting]").length')>0);
  };
  const capture = async name => {
    await mkdir('artifacts/brush-sculpt-web',{recursive:true});
    const shot = await call('Page.captureScreenshot',{format:'png'});
    await writeFile(`artifacts/brush-sculpt-web/${name}.png`,Buffer.from(shot.data,'base64'));
    await writeDrawerAudit(evaluate,`artifacts/brush-sculpt-web/${name}.allocations.json`,name);
  };
  const headerBarCorners = async () => {
    const saved = await evaluate('({workspace:layerApp.app.workspace_persistence(),transparency:layerApp.state().settings.transparency})');
    await evaluate('(()=>{const set=layerApp.app.set_glass;window.headerBarGlass={set,boxes:[]};layerApp.app.set_glass=function(boxes,...rest){headerBarGlass.boxes=Array.from(boxes);return set.call(this,boxes,...rest)};})()');
    const levels = ['off','low','medium','high'];
    await send({type:'preferences',action:{type:'edit',id:'transparency',value:levels.indexOf('medium')}});
    const corners = selector => evaluate(`(()=>{
      const source=document.querySelector(${JSON.stringify(selector)}),a=source.getBoundingClientRect();
      const bar=[...document.querySelectorAll('#header .header-bar:not([hidden])')].find(node=>{const b=node.getBoundingClientRect();return b.x<=a.x+.5&&b.right>=a.right-.5&&Math.abs(b.y-a.y)<.5});
      const b=bar.getBoundingClientRect(),c=layerApp.canvas.getBoundingClientRect(),boxes=headerBarGlass.boxes;
      const box=Array.from({length:boxes.length/9},(_,i)=>boxes.slice(i*9,i*9+9)).find(r=>[r[0]+c.x-b.x,r[1]+c.y-b.y,r[2]-b.width,r[3]-b.height].every(v=>Math.abs(v)<.5));
      const rect=r=>({x:r.x,y:r.y,width:r.width,height:r.height}),facing=source.dataset.drawerFacing;
      return {
        css:['borderTopLeftRadius','borderTopRightRadius','borderBottomRightRadius','borderBottomLeftRadius'].map(key=>parseFloat(getComputedStyle(bar)[key])),
        glass:box?.slice(4,8)??null,
        joined:facing?layerApp.app.toolbar_ui({type:'drawer_source_corners',anchor:rect(a),direction:facing,container:rect(b)}):[false,false,false,false],
      };
    })()`);
    try {
      for(const end of [true,false]) {
        await evaluate(`(()=>{
          const workspace=layerApp.app.workspace_persistence(),h=workspace.layout.header;
          const tool=control=>({id:h.next_id++,item:{kind:'tool',control}}),color=tool({kind:'color'});
          window.headerBarColor=color.id;
          h.zones=[[tool({kind:'command',command:'pencil'}),color,...(${end}?[]:[tool({kind:'command',command:'eraser'})])],[],[]];
          layerApp.dispatch({type:'restore_workspace',workspace});
        })()`);
        await settle(); await pause();
        const source=`[data-header-item="${await evaluate('headerBarColor')}"] .header-tool`;
        const closed=await corners(source);
        assert.ok(closed.css.every(r=>r>0),`closed bar is round: ${closed.css}`);
        assert.deepEqual(closed.glass,closed.css,'closed bar glass follows its corners');
        await contact(source);
        assert.ok(await drawer(),'header Color drawer opens');
        const open=await corners(source);
        assert.equal(open.joined.includes(true),end,`only a bar end joins its drawer: ${open.joined}`);
        open.joined.forEach((joined,i)=>assert.equal(open.css[i]===0,joined,`bar corner ${i}: radius ${open.css[i]}, joined ${joined}`));
        assert.deepEqual(open.glass,open.css,'open bar glass follows its corners');
        await contact(source);
        assert.equal(await drawer(),undefined);
        const reclosed=await corners(source);
        assert.deepEqual(reclosed.css,closed.css,'closing restores the round bar');
        assert.deepEqual(reclosed.glass,closed.css,'closing restores the round bar glass');
      }
    } finally {
      await evaluate('layerApp.app.set_glass=headerBarGlass.set;delete window.headerBarGlass;delete window.headerBarColor');
      await send({type:'restore_workspace',workspace:saved.workspace});
      await send({type:'preferences',action:{type:'edit',id:'transparency',value:levels.indexOf(saved.transparency)}});
    }
  };
  try {
    await headerBarCorners();
    await send({type:'invoke',command:'drawing_brush'});
    await contact(header('drawing_brush'));
    await check('brush_sets');
    assert.deepEqual(await evaluate('layerApp.state().tool_panels.brush_sets.groups.map(s=>s.label)'),['Pen','Pencil','Pastel','Paint','Watercolor','Oil paint','Airbrush']);
    await evaluate('window.brushSetRoot=document.querySelector(".content-drawer .brush-sets-control")');
    for(const [device,label] of [['mouse','Pencil'],['touch','Pastel'],['pen','Paint']]) {
      assert.ok(await evaluate(`document.querySelector(${JSON.stringify(set('brush_sets',label))}).getBoundingClientRect().height>=44`));
      await contact(set('brush_sets',label),device); await check('brush_sets');
      assert.ok(await evaluate('brushSetRoot===document.querySelector(".content-drawer .brush-sets-control")'),'Set list is retained');
      const choice=await evaluate('layerApp.state().tool_set.subtools[0]');
      await contact(`.content-drawer .tools-control [data-brush="${choice.preview}"]`,device);
      assert.equal(await evaluate('layerApp.state().brush.preset'),choice.preview);
    }
    await send({type:'set_brush_size',value:37});
    const drawing=await evaluate('layerApp.state().brush.preset');
    for(const theme of ['light','dark']) {await send({type:'set_theme',theme}); await capture(`brush-${theme}`);}
    await contact(header('sculpt'),'pen'); await check('sculpt_sets');
    assert.deepEqual(await evaluate('layerApp.state().tool_panels.sculpt_sets.groups.map(s=>s.label)'),['Blend','Paint mixing','Effects','Liquify','Retouch']);
    for(const device of ['mouse','touch','pen']) for(const label of ['Liquify','Blend']) {
      await contact(set('sculpt_sets',label),device); await check('sculpt_sets');
      assert.equal(await evaluate('layerApp.state().brush.tool'),label.toLowerCase());
    }
    await contact(set('sculpt_sets','Liquify'),'touch');
    await send({type:'set_brush_size',value:79});
    const sculpt=await evaluate('layerApp.state().brush.preset');
    for(const theme of ['light','dark']) {await send({type:'set_theme',theme}); await capture(`sculpt-${theme}`);}
    await contact(header('drawing_brush'));
    assert.deepEqual(await evaluate('[layerApp.state().brush.preset,layerApp.state().brush.diameter]'),[drawing,37]);
    await contact(header('eraser'),'touch');
    assert.deepEqual((await drawer()).columns,[['tool_settings']]);
    assert.equal(await evaluate('document.querySelectorAll(".content-drawer .tool-groups").length'),0);
    await capture('eraser');
    assert.ok(await evaluate('layerApp.state().commands.filter(c=>["drawing_brush","sculpt"].includes(c.id)).every(c=>!c.selected)'));
    await contact(header('sculpt'),'pen');
    assert.deepEqual(await evaluate('[layerApp.state().brush.preset,layerApp.state().brush.diameter]'),[sculpt,79]);
    await contact(header('sculpt')); assert.equal(await drawer(),undefined);
    await contact(header('drawing_brush')); assert.equal(await drawer(),undefined,'First contact selects Brush');
    await contact(header('drawing_brush')); await check('brush_sets');
    console.log('PASS: Brush/Sculpt three-panel drawers, mouse/touch/pen contacts, filtered sets, retained lists, independent selections, settings, light/dark');
  } finally {
    await send({type:'set_theme',theme});
    await send({type:'workspace_manager',command:{type:'switch',id:original}});
    await wait('!JSON.parse(layerApp.app.workspace_view()).busy && !JSON.parse(layerApp.app.workspace_view()).dirty');
    await send({type:'workspace_manager',command:{type:'form',action:{type:'delete',value:created}}});
    await send({type:'workspace_manager',command:{type:'submit',name:''}});
    await evaluate('delete window.brushSetRoot');
  }
}
