// Hover is presentation only. Keep shared, dynamically updated title text and
// use one noninteractive top-layer tooltip for mouse and pen (never contact).
// A disabled control can also reveal its title on tap, for any device, until
// the next contact or REVEAL_MS.
export const REVEAL_MS = 4000;
const contents = new WeakMap();
let refreshContent = () => {};
export function plainTip(tip) {
  return [tip.text,...tip.lines.map(line=>`${line.label}: ${line.value}`)].filter(Boolean).join('\n');
}
export function setTooltipContent(node, tip) {
  const key=JSON.stringify(tip),previous=contents.get(node);
  if(previous?.key===key)return;
  const plain=plainTip(tip);
  contents.set(node,{key,tip,plain});
  if(previous?.plain!==plain)node.title=plain;
  refreshContent(node);
}
function renderContent(node,tip) {
  node.replaceChildren();
  if(tip.text){const text=document.createElement('div');text.className='tip-text';text.textContent=tip.text;node.append(text);}
  const grid=document.createElement('div');grid.className=`tip-lines${tip.figures?' tip-figures':''}`;
  for(const line of tip.lines){
    const key=document.createElement('span'),value=document.createElement('span');
    key.className='tip-key';key.textContent=line.label;value.textContent=line.value;grid.append(key,value);
  }
  node.append(grid);
}
let reveal = () => {};
export function revealTooltip(node) { reveal(node); }
export function installTooltips() {
  const tip = document.createElement("div");
  tip.id = "hover-tooltip"; tip.className = "hover-tooltip"; tip.popover = "manual";
  tip.setAttribute("role", "tooltip"); document.body.append(tip);
  let source = null, parent = null, text = "", timer = 0, dismissed = null;
  const observer = new MutationObserver(records => {
    if (!source) return;
    if (!source.isConnected || source.parentNode !== parent) { hide(); return; }
    const updated = source.getAttribute("title");
    if (updated) {
      const changed = text !== updated;
      text = updated; source.setAttribute("title", "");
      observer.takeRecords(); // Discard our own title suppression.
      if (changed && tip.matches(":popover-open")) show();
    } else if (records.some(r => r.type === "attributes" && r.target === source)) {
      text = ""; hide(); // The host cleared an obsolete hint, e.g. an input error.
    }
  });
  function hide() {
    clearTimeout(timer); timer = 0; observer.disconnect();
    if (tip.matches(":popover-open")) tip.hidePopover();
    if (source) {
      if (source.getAttribute("title") === "") source.setAttribute("title", text);
      const ids = (source.getAttribute("aria-describedby") || "").split(/\s+/).filter(id => id && id !== tip.id);
      if (ids.length) source.setAttribute("aria-describedby", ids.join(" "));
      else source.removeAttribute("aria-describedby");
    }
    source = parent = null; text = "";
  }
  function show() {
    timer = 0;
    if (!source?.isConnected || source.parentNode !== parent) { hide(); return; }
    const r = source.getBoundingClientRect(), viewport = window.visualViewport;
    const left = viewport?.offsetLeft || 0, top = viewport?.offsetTop || 0;
    const width = viewport?.width || innerWidth, height = viewport?.height || innerHeight;
    if (!r.width || !r.height || r.bottom <= top || r.top >= top + height || r.right <= left || r.left >= left + width) { hide(); return; }
    const content=contents.get(source)?.tip;
    if(content?.lines.length)renderContent(tip,content);else tip.textContent=text;
    tip.style.maxWidth = `${Math.min(content?.lines.length?width:320, width - 16)}px`;
    if (!tip.matches(":popover-open")) tip.showPopover();
    const b = tip.getBoundingClientRect();
    const x = Math.max(left + 8, Math.min(r.left + r.width / 2 - b.width / 2, left + width - b.width - 8));
    const y = r.bottom + 4 + b.height <= top + height - 8 ? r.bottom + 4 : Math.max(top + 8, r.top - b.height - 4);
    tip.style.left = `${x}px`; tip.style.top = `${y}px`;
    const ids = new Set((source.getAttribute("aria-describedby") || "").split(/\s+/).filter(Boolean));
    ids.add(tip.id); source.setAttribute("aria-describedby", [...ids].join(" "));
  }
  refreshContent = node => { if(node===source && tip.matches(":popover-open"))show(); };
  function follow(next) {
    source = next; parent = next.parentNode; text = next.getAttribute("title");
    // An empty title also suppresses inherited browser tooltips. Restore the
    // latest value on exit; the observer preserves pointerenter shortcut updates.
    source.setAttribute("title", "");
    observer.observe(source, { attributes: true, attributeFilter: ["title"] });
    observer.observe(document.body, { childList: true, subtree: true });
  }
  reveal = node => {
    hide();
    if (!node.getAttribute("title")) return;
    follow(node); show();
    timer = setTimeout(hide, REVEAL_MS);
  };
  function hover(e) {
    if (e.buttons || !["mouse", "pen"].includes(e.pointerType)) { hide(); return; }
    const next = e.target.closest?.("[title]");
    if (next !== dismissed) dismissed = null;
    if (next === source) return;
    hide();
    if (!next || next === dismissed || !next.getAttribute("title")) return;
    follow(next);
    timer = setTimeout(show, 500);
  }
  document.addEventListener("pointerover", hover, true);
  document.addEventListener("pointermove", hover, true);
  document.addEventListener("pointerout", e => {
    if (source && !source.contains(e.relatedTarget)) hide();
    if (dismissed && !dismissed.contains(e.relatedTarget)) dismissed = null;
  }, true);
  for (const event of ["pointerdown", "pointercancel", "scroll", "dragstart", "contextmenu"])
    window.addEventListener(event, hide, true);
  window.addEventListener("keydown", () => { dismissed = source; hide(); }, true);
  window.addEventListener("blur", hide);
  window.addEventListener("resize", hide);
  document.addEventListener("visibilitychange", hide);
  window.visualViewport?.addEventListener("resize", hide);
  window.visualViewport?.addEventListener("scroll", hide);
  tip.addEventListener("toggle", () => { if (!tip.matches(":popover-open") && !timer) hide(); });
}
