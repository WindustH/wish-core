// Evaluated in the current document. References follow the node, even if a list reorders.
(a) => {
  const body = document.body;
  const text = body?.innerText ?? "";
  const header = { url: location.href, title: document.title };
  if (a.action === "read") {
    let source = text;
    if (a.section) {
      const headings = [...document.querySelectorAll("h1,h2,h3,h4,h5,h6")];
      const i = headings.findIndex(h => h.innerText.trim() === a.section);
      if (i < 0) throw Error("section not found");
      const range = document.createRange();
      range.selectNodeContents(body);
      range.setStartBefore(headings[i]);
      const end = headings.slice(i + 1).find(h => h.tagName <= headings[i].tagName);
      if (end) range.setEndBefore(end);
      source = range.toString();
    }
    const start = a.offset || 0;
    return { ...header, text: source.slice(start, start + 8000), next_offset: start + 8000 < source.length ? start + 8000 : null };
  }
  if (a.action === "wait") return a.selector ? !!document.querySelector(a.selector) : text.includes(a.text);
  const state = window.__wishBrowserRefs ??= {
    prefix: crypto.getRandomValues(new Uint32Array(2)).join("-"),
    ids: new WeakMap(), nodes: new Map(), count: 0
  };
  if (a.action === "observe") {
    const elements = [];
    state.nodes.clear();
    const controls = document.querySelectorAll('a[href],button,input:not([type=hidden]),textarea,select,[contenteditable=true],[role=button],[role=link],[role=tab],[role=menuitem],[role=checkbox]');
    for (const el of controls) {
      const r = el.getBoundingClientRect();
      if (!r.width || !r.height || r.bottom < 0 || r.top > innerHeight || getComputedStyle(el).visibility === 'hidden') continue;
      let ref = state.ids.get(el);
      if (!ref) { ref = state.prefix + "-" + ++state.count; state.ids.set(el, ref); }
      state.nodes.set(ref, el);
      elements.push({ ref, role: el.getAttribute("role") || el.tagName.toLowerCase(),
        name: (el.getAttribute("aria-label") || el.labels?.[0]?.innerText || el.innerText || el.placeholder || el.title || "").trim().slice(0,160),
        disabled: el.matches(":disabled,[aria-disabled=true]") || undefined });
      if (elements.length >= 100) break;
    }
    return { ...header, text: text.slice(0,1600), elements, viewport: [innerWidth,innerHeight] };
  }
  const el = state.nodes.get(a.ref);
  if (!el?.isConnected) throw Error("stale_ref: observe the current page");
  if (el.matches(":disabled,[aria-disabled=true]")) throw Error("element is disabled");
  el.scrollIntoView({block:"nearest",inline:"nearest",behavior:"instant"});
  const rect = el.getBoundingClientRect();
  const x = (Math.max(0, rect.left) + Math.min(innerWidth, rect.right)) / 2;
  const y = (Math.max(0, rect.top) + Math.min(innerHeight, rect.bottom)) / 2;
  if (!el.contains(document.elementFromPoint(x,y))) throw Error("element is obscured");
  if (a.action === "click") return {x,y};
  el.focus();
  if (a.action === "select") {
    if (!(el instanceof HTMLSelectElement) || ![...el.options].some(o => o.value === a.value)) throw Error("option not found");
    el.value = a.value;
    for (const type of ["input","change"]) el.dispatchEvent(new Event(type,{bubbles:true}));
  } else if (a.action === "fill") {
    if (el.readOnly) throw Error("field is read-only");
    if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) el.select();
    else if (el.isContentEditable) {
      const range = document.createRange(); range.selectNodeContents(el);
      const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range);
    } else throw Error("element is not editable");
  }
  return true;
}
