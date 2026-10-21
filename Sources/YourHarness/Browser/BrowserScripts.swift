import WebKit

/// The JavaScript the browser injects. The agent's helpers run in their own content world,
/// so the page can't see, call or spoof them. Only the console and network hooks run in
/// the page's world, because that's where `console` and `fetch` are.
@MainActor
enum BrowserScripts {
    static let world = WKContentWorld.world(name: "YourHarness")
    /// Script messages from the page's world: console lines and requests.
    static let logHandler = "yourHarnessLog"

    static func install(in controller: WKUserContentController) {
        controller.addUserScript(WKUserScript(source: helpers, injectionTime: .atDocumentStart, forMainFrameOnly: true, in: world))
        controller.addUserScript(WKUserScript(source: hooks, injectionTime: .atDocumentStart, forMainFrameOnly: true, in: .page))
    }

    /// `window.__yh` in our world: the page outline with refs, and what actions need.
    static let helpers = #"""
    (() => {
      if (window.__yh) return;
      let refs = new Map(), ids = new WeakMap(), next = 1, assigning = true;
      const MAX_NAME = 100, MAX_TEXT = 160;
      const clean = (s) => (s || '').replace(/\s+/g, ' ').trim();
      const clip = (s, n) => s.length > n ? s.slice(0, n - 1) + '…' : s;
      const interactiveRoles = new Set(['link','button','checkbox','radio','textbox','searchbox','combobox','listbox','option',
        'slider','spinbutton','switch','tab','menuitem','menuitemcheckbox','menuitemradio','treeitem','clickable']);
      // Their name is their text: what's inside is the name, not more lines.
      const nameFromContent = new Set(['link','button','heading','option','tab','menuitem','menuitemcheckbox','menuitemradio',
        'treeitem','switch','checkbox','radio','clickable','cell','columnheader','rowheader','listitem','status','alert']);
      const containers = new Set(['banner','navigation','main','contentinfo','complementary','region','search','form','dialog',
        'alertdialog','list','listitem','table','row','group','tablist','menu','menubar','tree','grid','toolbar','article']);
      const inlineRoles = new Set(['alert','status']);
      const landmarkTags = { nav: 'navigation', main: 'main', aside: 'complementary', search: 'search', article: 'article' };

      const dataTables = new WeakMap();
      function isDataTable(table) {
        if (!table) return false;
        if (!dataTables.has(table)) {
          const r = table.getAttribute('role');
          dataTables.set(table, r === 'table' || r === 'grid' || !!table.querySelector(':scope > caption, :scope > thead, :scope > tbody > tr > th, :scope > tr > th'));
        }
        return dataTables.get(table);
      }

      function isSectioned(el) {
        return !!el.parentElement?.closest('article,aside,main,nav,section');
      }

      function role(el) {
        const explicit = clean(el.getAttribute('role')).split(' ')[0];
        if (explicit && explicit !== 'none' && explicit !== 'presentation' && explicit !== 'generic') return explicit;
        if (explicit === 'none' || explicit === 'presentation') return null;
        const tag = el.localName;
        switch (tag) {
          case 'a': case 'area': return el.hasAttribute('href') ? 'link' : null;
          case 'button': return 'button';
          case 'summary': return 'button';
          case 'input': {
            const t = (el.getAttribute('type') || 'text').toLowerCase();
            if (t === 'hidden') return null;
            const map = { button: 'button', submit: 'button', reset: 'button', image: 'button', file: 'button', color: 'button',
              checkbox: 'checkbox', radio: 'radio', range: 'slider', number: 'spinbutton' };
            if (map[t]) return map[t];
            if (el.hasAttribute('list')) return 'combobox';
            return t === 'search' ? 'searchbox' : 'textbox';
          }
          case 'textarea': return 'textbox';
          case 'select': return el.multiple || el.size > 1 ? 'listbox' : 'combobox';
          case 'option': return 'option';
          case 'h1': case 'h2': case 'h3': case 'h4': case 'h5': case 'h6': return 'heading';
          case 'img': return el.getAttribute('alt') === '' ? null : 'img';
          case 'svg': return el.getAttribute('aria-label') ? 'img' : null;
          case 'header': return isSectioned(el) ? null : 'banner';
          case 'footer': return isSectioned(el) ? null : 'contentinfo';
          case 'form': return 'form';
          case 'section': return el.hasAttribute('aria-label') || el.hasAttribute('aria-labelledby') ? 'region' : null;
          case 'dialog': return el.open ? 'dialog' : null;
          case 'ul': case 'ol': case 'menu': return 'list';
          case 'li': return 'listitem';
          // Tables used for layout (no headers, no caption) are just boxes.
          case 'table': return isDataTable(el) ? 'table' : null;
          case 'tr': return isDataTable(el.closest('table')) ? 'row' : null;
          case 'td': return isDataTable(el.closest('table')) ? 'cell' : null;
          case 'th': return isDataTable(el.closest('table')) ? 'columnheader' : null;
          case 'fieldset': case 'details': return 'group';
          case 'progress': return 'progressbar';
          case 'iframe': return 'iframe';
          case 'video': return 'video';
          case 'audio': return 'audio';
        }
        if (landmarkTags[tag]) return landmarkTags[tag];
        if (el.isContentEditable && (el.parentElement == null || !el.parentElement.isContentEditable)) return 'textbox';
        return null;
      }

      function labelledText(el) {
        const ids = clean(el.getAttribute('aria-labelledby'));
        if (!ids) return '';
        return clean(ids.split(' ').map(id => el.ownerDocument.getElementById(id)?.textContent || '').join(' '));
      }

      function name(el, r) {
        let n = labelledText(el) || clean(el.getAttribute('aria-label'));
        if (!n && (el.localName === 'input' || el.localName === 'textarea' || el.localName === 'select')) {
          const t = (el.getAttribute('type') || '').toLowerCase();
          if (['button', 'submit', 'reset'].includes(t)) n = clean(el.value) || (t === 'submit' ? 'Submit' : t === 'reset' ? 'Reset' : '');
          if (!n && el.labels?.length) n = clean([...el.labels].map(l => l.innerText || l.textContent).join(' '));
          if (!n) n = clean(el.getAttribute('placeholder'));
          if (!n && t === 'image') n = clean(el.getAttribute('alt'));
        }
        if (!n && (el.localName === 'img' || el.localName === 'area')) n = clean(el.getAttribute('alt'));
        if (!n && el.localName === 'svg') n = clean(el.querySelector('title')?.textContent);
        // A box of links and buttons is named by them, line by line, not by all its text at once.
        const boxy = r === 'cell' || r === 'listitem' || r === 'columnheader' || r === 'rowheader' || r === 'status' || r === 'alert';
        if (!n && nameFromContent.has(r) && !(boxy && el.querySelector('a[href],button,input,select,textarea,[role=button],[role=link]'))) {
          n = clean(el.innerText ?? el.textContent);
        }
        if (!n && r === 'iframe') n = clean(el.getAttribute('title') || el.getAttribute('src'));
        if (!n) n = clean(el.getAttribute('title'));
        if (!n && (r === 'link' || r === 'button')) {
          const img = el.querySelector('img[alt],svg[aria-label]');
          n = clean(img?.getAttribute('alt') || img?.getAttribute('aria-label'));
        }
        return clip(n, MAX_NAME);
      }

      function visible(el) {
        if (el.getAttribute('aria-hidden') === 'true') return false;
        if (el.checkVisibility && !el.checkVisibility({ checkVisibilityCSS: true })) return false;
        return true;
      }

      function hasBox(el) {
        const r = el.getBoundingClientRect();
        return r.width > 0 && r.height > 0;
      }

      function refFor(el) {
        let id = ids.get(el);
        // Looking without acting (the side chat): existing refs only, the agent's stay valid.
        if (!assigning) return id;
        if (!id) {
          id = 'e' + next++;
          ids.set(el, id);
        }
        refs.set(id, new WeakRef(el));
        return id;
      }

      function inView(el) {
        const r = el.getBoundingClientRect();
        return r.bottom > 0 && r.right > 0 && r.top < innerHeight && r.left < innerWidth;
      }

      function href(el) {
        const raw = el.getAttribute('href');
        if (!raw || raw.startsWith('javascript:')) return '';
        try {
          const url = new URL(raw, location.href);
          return url.origin === location.origin ? url.pathname + url.search + url.hash : url.href;
        } catch { return raw; }
      }

      function states(el, r) {
        const out = [];
        if (r === 'heading') out.push('level=' + (el.getAttribute('aria-level') || el.localName.slice(1)));
        const checked = el.getAttribute('aria-checked') ?? (('checked' in el && (el.type === 'checkbox' || el.type === 'radio')) ? String(el.checked) : null);
        if (checked === 'true') out.push('checked');
        if (checked === 'mixed') out.push('mixed');
        if (el.disabled || el.getAttribute('aria-disabled') === 'true') out.push('disabled');
        const expanded = el.getAttribute('aria-expanded');
        if (expanded) out.push(expanded === 'true' ? 'expanded' : 'collapsed');
        if (el.getAttribute('aria-selected') === 'true' || (el.localName === 'option' && el.selected)) out.push('selected');
        if (el.getAttribute('aria-pressed') === 'true') out.push('pressed');
        if (el.getAttribute('aria-current') && el.getAttribute('aria-current') !== 'false') out.push('current');
        if (el.required || el.getAttribute('aria-required') === 'true') out.push('required');
        if (el.getAttribute('aria-invalid') === 'true' || (el.validity && el.willValidate && !el.validity.valid && el.value)) out.push('invalid');
        if (el === el.ownerDocument.activeElement && el !== el.ownerDocument.body) out.push('focused');
        return out;
      }

      function value(el, r) {
        if (el.localName === 'select') return clean([...el.selectedOptions].map(o => o.label).join(', '));
        if (r === 'textbox' || r === 'searchbox' || r === 'combobox' || r === 'spinbutton' || r === 'slider') {
          if (el.isContentEditable) return clip(clean(el.innerText), MAX_TEXT);
          if (el.type === 'password') return el.value ? '••••••' : '';
          return clip(el.value ?? '', MAX_TEXT);
        }
        return null;
      }

      function clickable(el) {
        if (el.hasAttribute('onclick') || (el.tabIndex >= 0 && el.hasAttribute('tabindex'))) return true;
        const style = getComputedStyle(el);
        if (style.cursor !== 'pointer') return false;
        const parent = el.parentElement;
        return !parent || getComputedStyle(parent).cursor !== 'pointer';
      }

      // A tree of what matters: roles, names, refs and text, flattening anonymous boxes.
      function build(root) {
        const out = [];
        let text = '';
        const flushText = (into) => {
          const t = clean(text);
          text = '';
          // Separators between links ("|", "·", "(") say nothing.
          if (t && /[\p{L}\p{N}]/u.test(t)) into.push({ text: t });
        };
        function walk(node, into) {
          if (node.nodeType === Node.TEXT_NODE) {
            text += node.data;
            return;
          }
          if (node.nodeType !== Node.ELEMENT_NODE) return;
          const el = node;
          if (['script', 'style', 'noscript', 'template', 'head', 'meta', 'link'].includes(el.localName)) return;
          if (!visible(el)) return;
          let r = role(el);
          if (!r && clickable(el) && clean(el.innerText).length) r = 'clickable';
          const block = !['inline', 'contents'].includes(getComputedStyle(el).display);
          if (block) flushText(into);
          if (r && (interactiveRoles.has(r) || r === 'heading' || r === 'img' || containers.has(r) || inlineRoles.has(r) ||
                    r === 'cell' || r === 'columnheader' || r === 'rowheader' || r === 'iframe' || r === 'progressbar' || r === 'dialog')) {
            flushText(into);
            if (!hasBox(el) && !containers.has(r)) return;
            const item = { role: r, name: name(el, r), states: states(el, r), children: [], el, inView: inView(el) };
            if (interactiveRoles.has(r) || r === 'heading' || r === 'img' || r === 'iframe') item.ref = refFor(el);
            const v = value(el, r);
            if (v !== null) item.value = v;
            if (r === 'link') item.href = href(el);
            const skipChildren = nameFromContent.has(r) && !(el.querySelector('a[href],button,input,select,textarea,[role=button],[role=link]'));
            if (!skipChildren && !['textbox', 'searchbox', 'combobox', 'listbox', 'img'].includes(r)) {
              for (const child of el.shadowRoot ? [...el.shadowRoot.childNodes, ...el.childNodes] : el.childNodes) walk(child, item.children);
              flushText(item.children);
              if (nameFromContent.has(r)) item.children = item.children.filter(c => !c.text);
            }
            into.push(item);
            return;
          }
          const kids = el.shadowRoot ? [...el.shadowRoot.childNodes, ...el.childNodes] : el.childNodes;
          if (el.localName === 'label' && el.control && visible(el.control)) {
            // Its text is its field's name, already on the field's line.
            const inner = [];
            for (const child of kids) walk(child, inner);
            text = '';
            for (const item of inner) if (!item.text) into.push(item);
            return;
          }
          for (const child of kids) walk(child, into);
          if (block) flushText(into);
        }
        walk(root, out);
        flushText(out);
        return out;
      }

      function quote(s) { return '"' + s.replace(/"/g, '\\"') + '"'; }

      // Lines with a priority each; the least useful go first when it's too long.
      function lines(items, depth, acc) {
        for (const item of items) {
          const pad = '  '.repeat(depth);
          if (item.text) {
            acc.push({ line: pad + '- text: ' + clip(item.text, MAX_TEXT), full: item.text, depth, kind: 'text' });
            continue;
          }
          // A box around one thing that says the same: just the thing.
          if (!item.ref && item.children.length === 1 && !item.children[0].text && (!item.name || item.name === item.children[0].name) &&
              ['listitem', 'cell', 'row', 'group', 'columnheader'].includes(item.role)) {
            lines(item.children, depth, acc);
            continue;
          }
          if (item.role === 'cell' && !item.name && item.children.length === 0) continue;
          // Anonymous containers with nothing useful are noise.
          if (containers.has(item.role) && !item.name && !item.ref) {
            if (item.children.length === 0) continue;
            if (item.children.length === 1 && item.children[0].text) {
              acc.push({ line: pad + '- ' + item.role + ': ' + clip(item.children[0].text, MAX_TEXT), depth, kind: 'text', inView: item.inView });
              continue;
            }
            if (['group', 'list', 'row', 'listitem', 'article'].includes(item.role) && item.children.length === 1) {
              lines(item.children, depth, acc);
              continue;
            }
          }
          let line = pad + '- ' + item.role;
          if (item.name) line += ' ' + quote(item.name);
          for (const s of item.states) line += ' [' + s + ']';
          if (item.ref) line += ' [ref=' + item.ref + ']';
          if (item.href) line += ' → ' + clip(item.href, 120);
          if (item.value !== undefined && item.value !== '') line += ': ' + quote(item.value);
          if (item.children.length) line += ':';
          const kind = interactiveRoles.has(item.role) ? 'interactive' : item.role === 'heading' ? 'heading' : containers.has(item.role) ? 'container' : 'other';
          acc.push({ line, depth, kind, inView: item.inView });
          lines(item.children, depth + 1, acc);
        }
        return acc;
      }

      function fit(all, budget) {
        const size = (ls) => ls.reduce((n, l) => n + l.line.length + 1, 0);
        let kept = all, dropped = 0;
        // Least useful first: text out of view, then any text, then out-of-view elements.
        const passes = [
          l => l.kind === 'text' && l.inView === false,
          l => l.kind === 'text',
          l => l.kind === 'other' && !l.inView,
          l => l.kind === 'interactive' && !l.inView,
          l => l.kind === 'container' && !l.inView,
        ];
        for (const drop of passes) {
          if (size(kept) <= budget) break;
          // Drop from the end of the page, so what's near the top survives.
          for (let i = kept.length - 1; i >= 0 && size(kept) > budget; i--) {
            if (drop(kept[i])) { kept = kept.slice(0, i).concat(kept.slice(i + 1)); dropped++; }
          }
        }
        if (size(kept) > budget) {
          let total = 0, i = 0;
          for (; i < kept.length && total + kept[i].line.length + 1 <= budget; i++) total += kept[i].line.length + 1;
          dropped += kept.length - i;
          kept = kept.slice(0, i);
        }
        return { kept, dropped };
      }

      const yh = {};

      yh.reset = (start) => {
        refs = new Map();
        ids = new WeakMap();
        next = start;
      };
      yh.nextRef = () => next;

      yh.snapshot = ({ selector, budget, keepRefs }) => {
        const root = selector ? document.querySelector(selector) : document.body;
        if (!root) throw new Error('No element matches ' + selector);
        assigning = !keepRefs;
        if (assigning) refs = new Map();
        const all = lines(build(root), 0, []);
        assigning = true;
        // Lines inside a dropped container stay readable: their indent says where they go.
        const { kept, dropped } = fit(all, budget);
        const se = document.scrollingElement || document.documentElement;
        return {
          outline: kept.map(l => l.line).join('\n'),
          dropped,
          title: document.title,
          url: location.href,
          viewport: [innerWidth, innerHeight],
          scroll: [Math.round(se.scrollLeft), Math.round(se.scrollTop)],
          size: [se.scrollWidth, se.scrollHeight],
          nextRef: next,
        };
      };

      yh.element = (target) => {
        if (target.ref) {
          const el = refs.get(target.ref)?.deref();
          if (!el || !el.isConnected) throw new Error('stale:' + target.ref);
          return el;
        }
        if (target.selector) {
          const el = document.querySelector(target.selector);
          if (!el) throw new Error('No element matches ' + target.selector);
          return el;
        }
        if (target.text) {
          const wanted = clean(target.text).toLowerCase();
          const candidates = [...document.querySelectorAll('a[href],button,input:not([type=hidden]),select,textarea,summary,[contenteditable=""],[contenteditable=true],[role],[onclick],[tabindex]')]
            .filter(el => visible(el) && hasBox(el));
          const named = candidates.map(el => [el, (name(el, role(el) || 'button') || '').toLowerCase()]);
          const hit = named.find(([, n]) => n === wanted) || named.find(([, n]) => n.startsWith(wanted)) || named.find(([, n]) => n.includes(wanted));
          if (hit) return hit[0];
          // A label names its field: the field is what's meant.
          for (const label of document.querySelectorAll('label')) {
            if (clean(label.innerText).toLowerCase().includes(wanted) && label.control) return label.control;
          }
          throw new Error('Nothing on the page is labelled "' + target.text + '". Take a snapshot and use a ref.');
        }
        return document.activeElement || document.body;
      };

      yh.describe = (el) => {
        const r = role(el) || (clickable(el) ? 'clickable' : el.localName);
        const n = name(el, r);
        return n ? r + ' ' + quote(clip(n, 60)) : r;
      };

      // Scrolls it into view and finds a point on it that a click would really hit.
      yh.point = (target) => {
        const el = yh.element(target);
        if (!el.isConnected) throw new Error('The element is gone from the page.');
        if (el.disabled) throw new Error(yh.describe(el) + ' is disabled.');
        let r = el.getBoundingClientRect();
        if (r.width === 0 || r.height === 0) throw new Error(yh.describe(el) + ' has no size: it is hidden or collapsed.');
        if (r.top < 0 || r.left < 0 || r.bottom > innerHeight || r.right > innerWidth) {
          el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
          r = el.getBoundingClientRect();
        }
        const candidates = [[0.5, 0.5], [0.25, 0.5], [0.75, 0.5], [0.5, 0.25], [0.5, 0.75], [0.1, 0.5]];
        let blocker = null;
        for (const [fx, fy] of candidates) {
          const x = Math.min(Math.max(r.left + r.width * fx, 0), innerWidth - 1);
          const y = Math.min(Math.max(r.top + r.height * fy, 0), innerHeight - 1);
          const hit = document.elementFromPoint(x, y);
          if (hit && (hit === el || el.contains(hit) || hit.contains(el) || (el.labels && [...el.labels].includes(hit)) || hit.closest('label')?.control === el)) {
            return { x, y, rect: [r.left, r.top, r.width, r.height], describe: yh.describe(el) };
          }
          blocker = blocker || hit;
        }
        return { x: r.left + r.width / 2, y: r.top + r.height / 2, rect: [r.left, r.top, r.width, r.height], describe: yh.describe(el),
                 blockedBy: blocker ? yh.describe(blocker) : 'something' };
      };

      yh.rect = (target) => {
        const r = yh.element(target).getBoundingClientRect();
        return [r.left, r.top, r.width, r.height];
      };

      // Focus, and select what's there so typing replaces it.
      const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
      yh.isExpression = (source) => {
        try { new AsyncFunction('return (\n' + source + '\n);'); return true; } catch { return false; }
      };

      yh.selectAll = () => {
        const el = document.activeElement;
        if (el && typeof el.select === 'function' && el.localName !== 'select') return el.select();
        const range = document.createRange();
        range.selectNodeContents(el && el.isContentEditable ? el : document.body);
        getSelection().removeAllRanges();
        getSelection().addRange(range);
      };

      yh.selectedText = () => {
        const el = document.activeElement;
        if (el && typeof el.selectionStart === 'number' && el.type !== 'password') return el.value.slice(el.selectionStart, el.selectionEnd);
        return String(getSelection());
      };

      yh.isSecret = (target) => yh.element(target).type === 'password';

      yh.prepareTyping = (target, clear) => {
        const el = yh.element(target);
        if (document.activeElement !== el) el.focus();
        if (clear) {
          if (typeof el.select === 'function' && el.localName !== 'select') el.select();
          else if (el.isContentEditable) {
            const range = document.createRange();
            range.selectNodeContents(el);
            const sel = getSelection();
            sel.removeAllRanges();
            sel.addRange(range);
          }
        }
        return yh.describe(el);
      };

      // Clears with a real edit, so frameworks see the change.
      yh.clear = (target) => {
        const el = yh.element(target);
        if (el.value !== undefined && el.value !== '' && typeof el.select === 'function') {
          el.select();
          document.execCommand('delete');
        }
      };

      yh.value = (target) => {
        const el = yh.element(target);
        return el.isContentEditable ? el.innerText : (el.value ?? null);
      };

      yh.click = (target) => {
        const el = yh.element(target);
        el.click();
        return yh.describe(el);
      };

      yh.select = (target, values) => {
        const el = yh.element(target);
        if (el.localName !== 'select') throw new Error(yh.describe(el) + ' is not a <select>; click it and pick the option instead.');
        const wanted = values.map(v => String(v).toLowerCase());
        const picked = [];
        for (const o of el.options) {
          const hit = wanted.includes(o.value.toLowerCase()) || wanted.includes(clean(o.label).toLowerCase());
          if (hit && !el.multiple && picked.length) continue;
          o.selected = hit;
          if (hit) picked.push(clean(o.label));
        }
        if (!picked.length) throw new Error('No option ' + values.join(', ') + '. Options: ' + [...el.options].map(o => clean(o.label)).join(', '));
        el.dispatchEvent(new Event('input', { bubbles: true }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
        return picked;
      };

      function scroller(el) {
        for (let n = el; n && n !== document.body; n = n.parentElement) {
          const s = getComputedStyle(n);
          if (/(auto|scroll)/.test(s.overflowY + s.overflowX) && (n.scrollHeight > n.clientHeight || n.scrollWidth > n.clientWidth)) return n;
        }
        return document.scrollingElement || document.documentElement;
      }

      yh.scroll = ({ target, direction, amount, to }) => {
        if (target && !direction && !to) {
          const el = yh.element(target);
          el.scrollIntoView({ block: 'center', behavior: 'instant' });
          return yh.describe(el) + ' is in view';
        }
        const box = target ? scroller(yh.element(target)) : (document.scrollingElement || document.documentElement);
        if (to === 'top') box.scrollTo({ top: 0, behavior: 'instant' });
        else if (to === 'bottom') box.scrollTo({ top: box.scrollHeight, behavior: 'instant' });
        else {
          const page = box === document.scrollingElement || box === document.documentElement;
          const step = amount || Math.round((page ? innerHeight : box.clientHeight) * 0.8);
          const [dx, dy] = { up: [0, -step], down: [0, step], left: [-step, 0], right: [step, 0] }[direction || 'down'];
          box.scrollBy({ left: dx, top: dy, behavior: 'instant' });
        }
        return 'Scrolled to ' + Math.round(box.scrollLeft) + ',' + Math.round(box.scrollTop) + ' of ' + box.scrollWidth + '×' + box.scrollHeight;
      };

      // Resolves once everything asked for is there (or gone), or rejects at the timeout.
      yh.waitFor = ({ text, selector, gone, timeout }) => new Promise((resolve, reject) => {
        const deadline = performance.now() + timeout;
        const check = () => {
          const body = document.body?.innerText || '';
          const textOK = !text || body.toLowerCase().includes(text.toLowerCase());
          const selOK = !selector || [...document.querySelectorAll(selector)].some(e => visible(e) && hasBox(e));
          const goneOK = !gone || !body.toLowerCase().includes(gone.toLowerCase());
          if (textOK && selOK && goneOK) return resolve(true);
          if (performance.now() > deadline) {
            const missing = [];
            if (!textOK) missing.push('text "' + text + '"');
            if (!selOK) missing.push('selector ' + selector);
            if (!goneOK) missing.push('"' + gone + '" to disappear');
            return reject(new Error('Timed out waiting for ' + missing.join(' and ')));
          }
          setTimeout(check, 100);
        };
        check();
      });

      // Where you picked: what the composer chip says.
      yh.pickInfo = (el) => {
        const path = [];
        for (let n = el; n && n.nodeType === 1 && path.length < 5; n = n.parentElement) {
          let part = n.localName;
          if (n.id) { path.unshift(part + '#' + CSS.escape(n.id)); break; }
          const cls = [...n.classList].slice(0, 2).map(c => '.' + CSS.escape(c)).join('');
          part += cls;
          const parent = n.parentElement;
          if (parent) {
            const same = [...parent.children].filter(c => c.localName === n.localName);
            if (same.length > 1) part += ':nth-of-type(' + (same.indexOf(n) + 1) + ')';
          }
          path.unshift(part);
        }
        const r = el.getBoundingClientRect();
        return { describe: yh.describe(el), selector: path.join(' > '), html: clip(el.outerHTML.replace(/\s+/g, ' '), 600),
                 rect: [r.left, r.top, r.width, r.height] };
      };

      let picking = null;
      yh.startPicking = () => {
        if (picking) return;
        const box = document.createElement('div');
        box.style.cssText = 'position:fixed;pointer-events:none;z-index:2147483647;border:2px solid #0a84ff;background:rgba(10,132,255,.12);border-radius:3px;transition:all .06s';
        let current = null;
        const move = (e) => {
          current = e.target;
          const r = current.getBoundingClientRect();
          Object.assign(box.style, { left: r.left + 'px', top: r.top + 'px', width: r.width + 'px', height: r.height + 'px' });
          if (!box.isConnected) document.documentElement.appendChild(box);
        };
        const stop = (e) => { e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation(); };
        const click = (e) => {
          stop(e);
          const info = yh.pickInfo(e.target);
          yh.stopPicking();
          window.webkit.messageHandlers.yourHarnessPick.postMessage(info);
        };
        const key = (e) => { if (e.key === 'Escape') { stop(e); yh.stopPicking(); window.webkit.messageHandlers.yourHarnessPick.postMessage(null); } };
        picking = { box, move, click, stop, key };
        addEventListener('mousemove', move, true);
        addEventListener('click', click, true);
        addEventListener('mousedown', stop, true);
        addEventListener('mouseup', stop, true);
        addEventListener('keydown', key, true);
      };
      yh.stopPicking = () => {
        if (!picking) return;
        removeEventListener('mousemove', picking.move, true);
        removeEventListener('click', picking.click, true);
        removeEventListener('mousedown', picking.stop, true);
        removeEventListener('mouseup', picking.stop, true);
        removeEventListener('keydown', picking.key, true);
        picking.box.remove();
        picking = null;
      };

      window.__yh = yh;
    })();
    """#

    /// In the page's world, at document start: console lines, uncaught errors, requests.
    static let hooks = #"""
    (() => {
      if (window.__yhHooked) return;
      window.__yhHooked = true;
      const handler = window.webkit?.messageHandlers?.yourHarnessLog;
      if (!handler) return;
      const post = (m) => { try { handler.postMessage(m); } catch {} };
      const fmt = (a) => {
        try {
          if (typeof a === 'string') return a;
          if (a instanceof Error) return a.stack ? String(a) + '\n' + a.stack : String(a);
          if (typeof Node !== 'undefined' && a instanceof Node) return '<' + a.nodeName.toLowerCase() + '>';
          if (a === undefined) return 'undefined';
          if (typeof a === 'function') return 'ƒ ' + (a.name || 'anonymous');
          const s = JSON.stringify(a);
          return s === undefined ? String(a) : s.length > 2000 ? s.slice(0, 2000) + '…' : s;
        } catch { return String(a); }
      };
      for (const level of ['log', 'info', 'warn', 'error', 'debug']) {
        const original = console[level];
        console[level] = function (...args) {
          post({ kind: 'console', level, text: args.map(fmt).join(' ').slice(0, 8000) });
          return original.apply(this, args);
        };
      }
      addEventListener('error', (e) => {
        const t = e.target;
        if (t && t !== window) {
          // A resource that didn't load: an image, script, stylesheet, video.
          const url = String(t.currentSrc || t.src || t.href || t.data || '');
          // An empty or "#" source resolves to the page itself: not a request that failed.
          if (url && url.split('#')[0] !== location.href.split('#')[0]) post({ kind: 'request', method: 'GET', url, failure: 'Failed to load', type: t.localName });
        } else if (e instanceof ErrorEvent) {
          const where = e.filename ? ' (' + e.filename + ':' + e.lineno + ')' : '';
          post({ kind: 'console', level: 'error', text: 'Uncaught ' + (e.error ? fmt(e.error) : e.message) + where });
        }
      }, true);
      addEventListener('unhandledrejection', (e) => post({ kind: 'console', level: 'error', text: 'Unhandled rejection: ' + fmt(e.reason) }));

      const originalFetch = window.fetch;
      if (originalFetch) window.fetch = async function (input, init) {
        const start = performance.now();
        const method = String(init?.method || input?.method || 'GET').toUpperCase();
        const url = String(input?.url || input);
        try {
          const response = await originalFetch.apply(this, arguments);
          post({ kind: 'request', method, url: response.url || url, status: response.status, type: 'fetch', ms: performance.now() - start });
          return response;
        } catch (error) {
          if (error?.name !== 'AbortError') post({ kind: 'request', method, url, failure: String(error?.message || error), type: 'fetch', ms: performance.now() - start });
          throw error;
        }
      };
      const open = XMLHttpRequest.prototype.open, send = XMLHttpRequest.prototype.send;
      XMLHttpRequest.prototype.open = function (method, url) {
        this.__yh = { method: String(method).toUpperCase(), url: String(url) };
        return open.apply(this, arguments);
      };
      XMLHttpRequest.prototype.send = function () {
        const info = this.__yh, start = performance.now();
        if (info) this.addEventListener('loadend', () => post({
          kind: 'request', method: info.method, url: this.responseURL || info.url, status: this.status || undefined,
          failure: this.status === 0 ? 'Network error' : undefined, type: 'xhr', ms: performance.now() - start }));
        return send.apply(this, arguments);
      };
      try {
        new PerformanceObserver((list) => {
          for (const e of list.getEntries()) {
            if (e.initiatorType === 'fetch' || e.initiatorType === 'xmlhttprequest') continue;
            post({ kind: 'request', method: 'GET', url: e.name, status: e.responseStatus || undefined, type: e.initiatorType, ms: e.duration });
          }
        }).observe({ type: 'resource', buffered: true });
      } catch {}
    })();
    """#
}
