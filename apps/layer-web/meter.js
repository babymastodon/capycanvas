import {setTooltipContent,plainTip} from './tooltips.js';

export function meterChip(element) {
  const node=element('span','meter-chip');
  return {node,update(chip){if(node.hidden!==!chip)node.hidden=!chip;if(chip){if(node.textContent!==chip.label)node.textContent=chip.label;node.classList.toggle('warning',chip.warning);}}};
}

export function meterBar(element) {
  const node=element('div','meter-bar');node.setAttribute('aria-hidden','true');
  let shown;
  return {node,update(meter){
    if(node.hidden!==!meter)node.hidden=!meter;if(!meter)return;
    const key=JSON.stringify(meter);if(key===shown)return;shown=key;
    const track=at=>`color-mix(in srgb,var(--text) ${at<meter.limit?12:6}%,transparent)`;
    const stops=[...new Set([0,1,meter.limit,...meter.segments.flatMap(s=>[s.from,s.to])])].sort((a,b)=>a-b);
    const parts=stops.slice(0,-1).flatMap((from,i)=>{
      const to=stops[i+1],at=(from+to)/2,segment=meter.segments.findLast(s=>s.from<=at&&s.to>=at),base=track(at);
      const color=!segment?base:({fill:'var(--accent)',over:'var(--error)',soft:`color-mix(in srgb,var(--accent) 40%,${base})`,rest:`color-mix(in srgb,var(--text) 32%,${base})`})[segment.tone];
      return [`${color} ${from*100}%`,`${color} ${to*100}%`];
    });
    node.style.backgroundImage=`linear-gradient(to right,${parts.join(',')})`;
    node.style.setProperty('--limit',meter.limit);node.classList.toggle('limited',meter.limit<.999);
  }};
}

export function meterRow(element,control) {
  const node=element('div','meter-row'),line=element('div','meter-line'),lead=element('div','meter-lead'),label=element('span','meter-label'),value=element('span','numeric'),chip=meterChip(element),bar=meterBar(element);
  if(control){node.dataset.control=control;value.dataset.control=control.replace('-row-','-value-');bar.node.dataset.control=control.replace('-row-','-bar-');}
  lead.append(label,chip.node);line.append(lead,value);node.append(line,bar.node);
  return {node,update(row){
    if(label.textContent!==row.label)label.textContent=row.label;
    if(value.textContent!==row.value)value.textContent=row.value;
    value.classList.toggle('warning',!!row.warning);chip.update(row.chip);bar.update(row.meter);
    setTooltipContent(node,row.tip);const description=row.description??plainTip(row.tip);
    if(node.getAttribute('aria-description')!==description)node.setAttribute('aria-description',description);
  }};
}

export function meterSection(element) {
  const node=element('div','meter-section'),heading=element('div','meter-heading'),title=element('span','meter-title'),note=element('span','meter-note'),chip=meterChip(element),body=element('div','meter-rows');
  node.dataset.control='renderer-memory';heading.dataset.control='renderer-memory-heading';heading.append(title,note,chip.node);node.append(heading,body);
  const rows=[];
  return {node,update(section){
    if(title.textContent!==section.title)title.textContent=section.title;
    if(note.textContent!==section.note)note.textContent=section.note;
    chip.update(section.chip);setTooltipContent(heading,section.tip);heading.setAttribute('aria-description',plainTip(section.tip));
    while(rows.length>section.rows.length)rows.pop().node.remove();
    while(rows.length<section.rows.length){const row=meterRow(element,`renderer-memory-row-${rows.length}`);rows.push(row);body.append(row.node);}
    section.rows.forEach((row,i)=>rows[i].update(row));
  }};
}
