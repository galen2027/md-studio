(function() {
  var flags = window.__mdPreviewFeatureFlags || { math: false, mermaid: false };
  var mermaidSeq = 0;
  var anchorNavigationBound = false;

  function idle(fn) {
    return (window.requestIdleCallback || function(cb) { return setTimeout(cb, 0); })(fn);
  }

  function setFlags(needsMath, needsMermaid) {
    flags = { math: !!needsMath, mermaid: !!needsMermaid };
    window.__mdPreviewFeatureFlags = flags;
  }

  function hasClassInTree(el, cls) {
    while (el && el !== document.body) {
      if (el.classList && el.classList.contains(cls)) return true;
      el = el.parentElement;
    }
    return false;
  }

  function shouldSkipTextNode(node) {
    var el = node.parentElement;
    if (!el) return true;
    var tag = el.tagName;
    if (tag === 'CODE' || tag === 'PRE' || tag === 'KBD' || tag === 'SAMP' ||
        tag === 'SCRIPT' || tag === 'STYLE' || tag === 'TEXTAREA') return true;
    return hasClassInTree(el, 'katex') ||
      hasClassInTree(el, 'math') ||
      hasClassInTree(el, 'mdp-mark') ||
      hasClassInTree(el, 'mdp-mermaid') ||
      hasClassInTree(el, 'mdp-mermaid-error');
  }

  function isEscaped(text, index) {
    var count = 0;
    for (var i = index - 1; i >= 0 && text[i] === '\\'; i--) count++;
    return count % 2 === 1;
  }

  function findClosing(text, start, delim) {
    for (var i = start; i < text.length; i++) {
      if (delim === '$') {
        if (text[i] === '$' && !isEscaped(text, i) && text[i - 1] && !/\s/.test(text[i - 1])) {
          return i;
        }
      } else if (text.substr(i, delim.length) === delim && !isEscaped(text, i)) {
        return i;
      }
    }
    return -1;
  }

  function nextDelimiter(text, index) {
    if (text.substr(index, 2) === '$$' && !isEscaped(text, index)) {
      return { open: '$$', close: '$$', display: true };
    }
    if (text.substr(index, 2) === '\\[' && !isEscaped(text, index)) {
      return { open: '\\[', close: '\\]', display: true };
    }
    if (text.substr(index, 2) === '\\(' && !isEscaped(text, index)) {
      return { open: '\\(', close: '\\)', display: false };
    }
    if (text[index] === '$' && !isEscaped(text, index) && text[index + 1] &&
        text[index + 1] !== '$' && !/\s/.test(text[index + 1])) {
      return { open: '$', close: '$', display: false };
    }
    return null;
  }

  function mathParts(text) {
    var parts = [];
    var i = 0;
    var last = 0;
    while (i < text.length) {
      var delim = nextDelimiter(text, i);
      if (!delim) {
        i++;
        continue;
      }
      var bodyStart = i + delim.open.length;
      var closeAt = findClosing(text, bodyStart, delim.close);
      if (closeAt < 0) {
        i += delim.open.length;
        continue;
      }
      if (i > last) parts.push({ text: text.slice(last, i) });
      parts.push({
        math: text.slice(bodyStart, closeAt),
        display: delim.display
      });
      i = closeAt + delim.close.length;
      last = i;
    }
    if (!parts.length) return null;
    if (last < text.length) parts.push({ text: text.slice(last) });
    return parts;
  }

  function enhanceMath(root) {
    if (!flags.math || !window.katex) return;
    var mathNodes = root.querySelectorAll('.math.math-inline, .math.math-display');
    Array.prototype.forEach.call(mathNodes, function(el) {
      if (el.dataset.mdpMath === '1') return;
      var source = el.textContent;
      var display = el.classList.contains('math-display');
      el.dataset.mdpMath = '1';
      try {
        window.katex.render(source, el, {
          displayMode: display,
          throwOnError: false,
          strict: 'warn',
          trust: false
        });
      } catch (err) {
        el.className = 'mdp-math-error';
        el.textContent = source;
        el.title = err && err.message ? err.message : 'KaTeX render error';
      }
    });

    var walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
      acceptNode: function(node) {
        if (shouldSkipTextNode(node)) return NodeFilter.FILTER_REJECT;
        return /(\$|\\\(|\\\[)/.test(node.nodeValue) ?
          NodeFilter.FILTER_ACCEPT :
          NodeFilter.FILTER_REJECT;
      }
    });
    var nodes = [];
    while (walker.nextNode()) nodes.push(walker.currentNode);
    nodes.forEach(function(node) {
      var parts = mathParts(node.nodeValue);
      if (!parts) return;
      var frag = document.createDocumentFragment();
      parts.forEach(function(part) {
        if (part.text !== undefined) {
          frag.appendChild(document.createTextNode(part.text));
          return;
        }
        var span = document.createElement('span');
        try {
          window.katex.render(part.math, span, {
            displayMode: part.display,
            throwOnError: false,
            strict: 'warn',
            trust: false
          });
        } catch (err) {
          span.className = 'mdp-math-error';
          span.textContent = part.math;
          span.title = err && err.message ? err.message : 'KaTeX render error';
        }
        frag.appendChild(span);
      });
      node.parentNode.replaceChild(frag, node);
    });
  }

  function currentMermaidTheme() {
    return window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches ?
      'dark' :
      'default';
  }

  function renderMermaid(container, source) {
    if (!window.mermaid || !window.mermaid.render) return;
    var id = 'mdp-mermaid-' + (++mermaidSeq);
    try {
      window.mermaid.initialize({
        startOnLoad: false,
        securityLevel: 'strict',
        theme: currentMermaidTheme()
      });
      Promise.resolve(window.mermaid.render(id, source)).then(function(result) {
        container.innerHTML = result.svg || result;
      }).catch(function(err) {
        container.className = 'mdp-mermaid-error';
        container.textContent = source;
        container.title = err && err.message ? err.message : 'Mermaid render error';
      });
    } catch (err) {
      container.className = 'mdp-mermaid-error';
      container.textContent = source;
      container.title = err && err.message ? err.message : 'Mermaid render error';
    }
  }

  function enhanceMermaid(root) {
    if (!flags.mermaid || !window.mermaid) return;
    var nodes = root.querySelectorAll('pre > code.language-mermaid, pre > code.mermaid');
    Array.prototype.forEach.call(nodes, function(code) {
      var pre = code.parentElement;
      if (!pre || pre.dataset.mdpMermaid === '1') return;
      pre.dataset.mdpMermaid = '1';
      var source = code.textContent;
      var container = document.createElement('div');
      container.className = 'mdp-mermaid';
      container.dataset.mdpDiagramSource = source;
      ['data-source-start', 'data-source-end'].forEach(function(name) {
        if (pre.hasAttribute(name)) container.setAttribute(name, pre.getAttribute(name));
      });
      container.textContent = source;
      pre.parentNode.replaceChild(container, pre);
      renderMermaid(container, source);
    });
  }

  // ---- Mermaid lightbox --------------------------------------------------
  // Large diagrams are shrunk to fit the column and become unreadable. Click
  // one to open a viewer that scales the *vector* SVG (crisp at any zoom),
  // with zoom controls and original-image download (SVG / 3x PNG).
  var lightbox = null;
  var lightboxSvg = null;
  var lightboxNatural = { w: 0, h: 0 };
  var lightboxScale = 1;
  var lightboxFit = 1;
  var lightboxSpaceHeld = false;
  var lightboxPan = null;
  var lightboxSuppressClick = false;

  function lightboxLabels() {
    return window.__mdPreviewLightboxLabels || {
      zoomOut: 'Zoom out', zoomIn: 'Zoom in', fit: 'Fit', pan: 'Hold Space and drag',
      png: 'PNG', svg: 'SVG', close: 'Close'
    };
  }

  function ensureLightbox() {
    if (lightbox) return lightbox;
    var L = lightboxLabels();
    lightbox = document.createElement('div');
    lightbox.className = 'mdp-lightbox';
    lightbox.innerHTML =
      '<div class="mdp-lightbox-toolbar">' +
      '<button type="button" data-lb="out" title="' + L.zoomOut + '" aria-label="' + L.zoomOut + '">−</button>' +
      '<span class="mdp-lightbox-scale">100%</span>' +
      '<button type="button" data-lb="in" title="' + L.zoomIn + '" aria-label="' + L.zoomIn + '">+</button>' +
      '<button type="button" data-lb="fit">' + L.fit + '</button>' +
      '<span class="mdp-lightbox-pan-hint">' + L.pan + '</span>' +
      '<span class="mdp-lightbox-spacer"></span>' +
      '<button type="button" data-lb="png">' + L.png + '</button>' +
      '<button type="button" data-lb="svg">' + L.svg + '</button>' +
      '<button type="button" data-lb="close" title="' + L.close + '" aria-label="' + L.close + '">×</button>' +
      '</div>' +
      '<div class="mdp-lightbox-body"><div class="mdp-lightbox-canvas"></div></div>';
    document.body.appendChild(lightbox);
    lightbox.addEventListener('click', function(e) {
      if (lightboxSuppressClick) {
        lightboxSuppressClick = false;
        e.preventDefault();
        e.stopPropagation();
        return;
      }
      var btn = e.target && e.target.closest ? e.target.closest('[data-lb]') : null;
      if (btn) {
        lightboxAction(btn.getAttribute('data-lb'));
        return;
      }
      if (e.target === lightbox || e.target.classList.contains('mdp-lightbox-body')) {
        closeLightbox();
      }
    });
    lightbox.addEventListener('wheel', function(e) {
      if (e.target.closest('.mdp-diagram-editor')) return;
      if (!lightboxSvg) return;
      e.preventDefault();
      applyLightboxScale(lightboxScale * (e.deltaY < 0 ? 1.12 : 1 / 1.12));
    }, { passive: false });
    var body = lightbox.querySelector('.mdp-lightbox-body');
    body.addEventListener('pointerdown', function(e) {
      if (!lightboxSpaceHeld || e.button !== 0 || !lightboxSvg) return;
      lightboxPan = {
        id: e.pointerId,
        x: e.clientX,
        y: e.clientY,
        left: body.scrollLeft,
        top: body.scrollTop,
        moved: false,
      };
      body.classList.add('panning');
      body.setPointerCapture(e.pointerId);
      e.preventDefault();
    });
    body.addEventListener('pointermove', function(e) {
      if (!lightboxPan || lightboxPan.id !== e.pointerId) return;
      var dx = e.clientX - lightboxPan.x;
      var dy = e.clientY - lightboxPan.y;
      if (Math.abs(dx) > 2 || Math.abs(dy) > 2) lightboxPan.moved = true;
      body.scrollLeft = lightboxPan.left - dx;
      body.scrollTop = lightboxPan.top - dy;
      e.preventDefault();
    });
    function stopLightboxPan(e) {
      if (!lightboxPan || lightboxPan.id !== e.pointerId) return;
      lightboxSuppressClick = lightboxPan.moved;
      lightboxPan = null;
      body.classList.remove('panning');
      if (body.hasPointerCapture(e.pointerId)) body.releasePointerCapture(e.pointerId);
      e.preventDefault();
    }
    body.addEventListener('pointerup', stopLightboxPan);
    body.addEventListener('pointercancel', stopLightboxPan);
    return lightbox;
  }

  function openLightbox(container, refresh) {
    var svg = container.querySelector('svg');
    if (!svg) return;
    ensureLightbox();
    var canvas = lightbox.querySelector('.mdp-lightbox-canvas');
    canvas.textContent = '';
    lightboxSvg = svg.cloneNode(true);
    canvas.appendChild(lightboxSvg);
    var w = 0, h = 0;
    var vb = svg.getAttribute('viewBox') || '';
    if (vb) {
      var p = vb.trim().split(/[\s,]+/);
      w = parseFloat(p[2]);
      h = parseFloat(p[3]);
    }
    if (!w || !h) {
      w = svg.clientWidth || 800;
      h = svg.clientHeight || 600;
    }
    lightboxNatural = { w: w, h: h };
    lightbox.classList.add('open');
    if (!refresh) document.dispatchEvent(new CustomEvent('mdpreview-diagram-open', { detail: { container: container, lightbox: lightbox } }));
    // Big diagrams open fitted; small ones open at 100% (Codex-style).
    requestAnimationFrame(function() {
      fitLightbox();
    });
  }

  function fitLightbox() {
    if (!lightboxSvg) return;
    var body = lightbox.querySelector('.mdp-lightbox-body'), style = getComputedStyle(body);
    var availW = Math.max(80, body.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight) - 28);
    var availH = Math.max(80, body.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom) - 28);
    lightboxFit = Math.min(1, availW / lightboxNatural.w, availH / lightboxNatural.h);
    applyLightboxScale(lightboxFit);
  }

  function applyLightboxScale(scale) {
    if (!lightboxSvg) return;
    lightboxScale = Math.max(0.05, Math.min(8, scale));
    lightboxSvg.setAttribute('width', String(Math.round(lightboxNatural.w * lightboxScale)));
    lightboxSvg.setAttribute('height', String(Math.round(lightboxNatural.h * lightboxScale)));
    lightboxSvg.style.maxWidth = 'none';
    lightboxSvg.style.height = 'auto';
    lightbox.querySelector('.mdp-lightbox-scale').textContent = Math.round(lightboxScale * 100) + '%';
  }

  function prepareSvgForExport(source, width, height) {
    try {
      var svg;
      if (typeof source === 'string') {
        var host = document.createElement('div');
        host.innerHTML = source;
        svg = host.querySelector('svg');
      } else if (source && source.cloneNode) {
        svg = source.cloneNode(true);
      }
      if (!svg) return '';
      svg.setAttribute('xmlns', 'http://www.w3.org/2000/svg');
      svg.setAttribute('width', String(Math.max(1, Math.round(width))));
      svg.setAttribute('height', String(Math.max(1, Math.round(height))));
      svg.style.width = Math.max(1, Math.round(width)) + 'px';
      svg.style.height = Math.max(1, Math.round(height)) + 'px';
      svg.style.maxWidth = 'none';
      svg.style.maxHeight = 'none';
      // DOM + XML serialization normalizes foreignObject HTML (<br> -> <br/>).
      return new XMLSerializer().serializeToString(svg);
    } catch (_) {
      return '';
    }
  }

  function serializedLightboxSvg() {
    if (!lightboxSvg) return '';
    // Export at the SVG's natural dimensions, independent of current viewer zoom.
    return prepareSvgForExport(lightboxSvg, lightboxNatural.w, lightboxNatural.h);
  }

  function rasterizeLightboxSvg() {
    return new Promise(function(resolve) {
      var scale = 3; // 3x raster keeps the download crisp on hi-dpi screens
      var w = lightboxNatural.w, h = lightboxNatural.h;
      var targetW = Math.max(1, Math.ceil(w * scale));
      var targetH = Math.max(1, Math.ceil(h * scale));
      var text = prepareSvgForExport(lightboxSvg, targetW, targetH);
      if (!text || !w || !h || targetW > 16000 || targetH > 16000 || targetW * targetH > 32000000) {
        resolve(null);
        return;
      }
      var img = new Image();
      img.onload = function() {
        try {
          var canvas = document.createElement('canvas');
          canvas.width = targetW;
          canvas.height = targetH;
          var ctx = canvas.getContext('2d');
          ctx.fillStyle = '#ffffff';
          ctx.fillRect(0, 0, canvas.width, canvas.height);
          ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
          resolve(canvas.toDataURL('image/png'));
        } catch (_) {
          resolve(null);
        }
      };
      img.onerror = function() { resolve(null); };
      img.src = 'data:image/svg+xml;base64,' + btoa(unescape(encodeURIComponent(text)));
    });
  }

  function lightboxAction(action) {
    if (action === 'close') { closeLightbox(); return; }
    if (action === 'in') { applyLightboxScale(lightboxScale * 1.25); return; }
    if (action === 'out') { applyLightboxScale(lightboxScale / 1.25); return; }
    if (action === 'fit') { fitLightbox(); return; }
    if (action === 'svg' && window.ipc && window.ipc.postMessage) {
      window.ipc.postMessage('download-svg\n' + serializedLightboxSvg());
      return;
    }
    if (action === 'png' && window.ipc && window.ipc.postMessage) {
      rasterizeLightboxSvg().then(function(dataUrl) {
        if (dataUrl) window.ipc.postMessage('download-png\n' + dataUrl);
      });
    }
  }

  function closeLightbox() {
    lightboxSpaceHeld = false;
    lightboxPan = null;
    if (lightbox) {
      lightbox.classList.remove('open', 'pan-ready');
      var body = lightbox.querySelector('.mdp-lightbox-body');
      if (body) body.classList.remove('panning');
    }
  }

  // Shared by the Word exporter and the lightbox regression verifier.
  window.__mdPreviewPrepareSvgForExport = prepareSvgForExport;
  window.__mdPreviewOpenDiagram = openLightbox;
  window.__mdPreviewRefreshDiagram = function(container) { openLightbox(container, true); };
  window.__mdPreviewFitDiagram = fitLightbox;
  window.addEventListener('resize', function() { if (lightbox && lightbox.classList.contains('open')) fitLightbox(); });

  var lightboxBound = false;

  function bindLightbox() {
    if (lightboxBound) return;
    lightboxBound = true;
    document.addEventListener('click', function(e) {
      var box = e.target && e.target.closest ? e.target.closest('.mdp-mermaid') : null;
      if (box && box.querySelector('svg')) openLightbox(box);
    });
    document.addEventListener('keydown', function(e) {
      if (e.isComposing || e.keyCode === 229) return;
      if (e.code === 'Space' && lightbox && lightbox.classList.contains('open') &&
          !e.ctrlKey && !e.metaKey && !e.altKey && !e.target.closest('input,textarea,select,[contenteditable="true"]')) {
        lightboxSpaceHeld = true;
        lightbox.classList.add('pan-ready');
        e.preventDefault();
        return;
      }
      if (e.key === 'Escape' && lightbox && lightbox.classList.contains('open')) {
        e.stopPropagation();
        closeLightbox();
      }
    }, true);
    document.addEventListener('keyup', function(e) {
      if (e.code !== 'Space') return;
      lightboxSpaceHeld = false;
      if (lightbox) lightbox.classList.remove('pan-ready');
    }, true);
    window.addEventListener('blur', function() {
      lightboxSpaceHeld = false;
      lightboxPan = null;
      if (lightbox) {
        lightbox.classList.remove('pan-ready');
        var body = lightbox.querySelector('.mdp-lightbox-body');
        if (body) body.classList.remove('panning');
      }
    });
  }


  function enhanceTables(root) {
    var tables = root.querySelectorAll('table');
    Array.prototype.forEach.call(tables, function(table) {
      if (hasClassInTree(table, 'mdp-table-wrap')) return;
      var firstRow = table.querySelector('tr');
      var cellCount = firstRow ? firstRow.children.length : 0;
      if (cellCount < 4) return;

      var wrap = document.createElement('div');
      wrap.className = 'mdp-table-wrap';
      table.parentNode.insertBefore(wrap, table);
      wrap.appendChild(table);
    });
  }

  function alertClassFor(value) {
    var type = String(value || '').toLowerCase();
    if (type === 'note' || type === 'tip' || type === 'important' ||
        type === 'warning' || type === 'caution') {
      return 'markdown-alert-' + type;
    }
    return '';
  }

  function alertTypeFromClass(node) {
    var match = String(node && node.className || '').match(/\bmarkdown-alert-(note|tip|important|warning|caution)\b/i);
    return match ? match[1].toLowerCase() : '';
  }

  function alertLabelFor(type) {
    var labels = {
      note: 'Note',
      tip: 'Tip',
      important: 'Important',
      warning: 'Warning',
      caution: 'Caution'
    };
    return labels[type] || '';
  }

  function ensureAlertTitle(quote, type) {
    if (!quote || !type) return;
    var existing = quote.firstElementChild;
    if (existing && existing.classList && existing.classList.contains('markdown-alert-title')) return;

    var title = document.createElement('p');
    title.className = 'markdown-alert-title';
    title.textContent = alertLabelFor(type);
    quote.insertBefore(title, quote.firstChild);
  }

  function trimAlertLead(node) {
    while (node && node.firstChild) {
      var child = node.firstChild;
      if (child.nodeType === Node.TEXT_NODE && !child.nodeValue.trim()) {
        node.removeChild(child);
        continue;
      }
      if (child.nodeName === 'BR') {
        node.removeChild(child);
        continue;
      }
      break;
    }
  }

  function enhanceAlerts(root) {
    var quotes = root.querySelectorAll('blockquote');
    Array.prototype.forEach.call(quotes, function(quote) {
      var type = alertTypeFromClass(quote);
      var first = quote.firstElementChild || quote;
      var match = null;

      if (!type && first && first.firstChild && first.firstChild.nodeType === Node.TEXT_NODE) {
        match = first.firstChild.nodeValue.match(/^\s*\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]\s*/i);
        if (match) type = String(match[1]).toLowerCase();
      }

      if (!type) return;

      var cls = alertClassFor(type);
      if (!cls) return;

      quote.classList.add(cls);
      if (match) {
        first.firstChild.nodeValue = first.firstChild.nodeValue.slice(match[0].length);
        trimAlertLead(first);
        if (first !== quote && !first.children.length && !first.textContent.trim()) {
          quote.removeChild(first);
        }
        trimAlertLead(quote);
      }
      ensureAlertTitle(quote, type);
    });
  }

  function markParts(text) {
    var parts = [];
    var cursor = 0;
    while (cursor < text.length) {
      var open = text.indexOf('==', cursor);
      if (open < 0) break;
      var close = text.indexOf('==', open + 2);
      if (close < 0) break;
      var body = text.slice(open + 2, close);
      if (!body.trim()) break;
      if (open > cursor) parts.push({ text: text.slice(cursor, open) });
      parts.push({ mark: body });
      cursor = close + 2;
    }
    if (!parts.length) return null;
    if (cursor < text.length) parts.push({ text: text.slice(cursor) });
    return parts;
  }

  function enhanceMarks(root) {
    var walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
      acceptNode: function(node) {
        if (shouldSkipTextNode(node)) return NodeFilter.FILTER_REJECT;
        return node.nodeValue.indexOf('==') >= 0 ?
          NodeFilter.FILTER_ACCEPT :
          NodeFilter.FILTER_REJECT;
      }
    });
    var nodes = [];
    while (walker.nextNode()) nodes.push(walker.currentNode);
    nodes.forEach(function(node) {
      var parts = markParts(node.nodeValue);
      if (!parts) return;
      var frag = document.createDocumentFragment();
      parts.forEach(function(part) {
        if (part.text !== undefined) {
          frag.appendChild(document.createTextNode(part.text));
          return;
        }
        var mark = document.createElement('mark');
        mark.className = 'mdp-mark';
        mark.textContent = part.mark;
        frag.appendChild(mark);
      });
      node.parentNode.replaceChild(frag, node);
    });
  }

  function decodeFragment(value) {
    try {
      return decodeURIComponent(value);
    } catch (_) {
      return value;
    }
  }

  function updateHash(fragment) {
    try {
      if (window.history && window.history.pushState) {
        window.history.pushState(null, '', '#' + fragment);
        return;
      }
    } catch (_) {}

    try {
      window.location.hash = fragment;
    } catch (_) {}
  }

  function bindAnchorNavigation() {
    if (anchorNavigationBound) return;
    anchorNavigationBound = true;

    document.addEventListener('click', function(event) {
      var link = event.target && event.target.closest ?
        event.target.closest('#preview a[href]') :
        null;
      if (!link) return;

      var href = link.getAttribute('href') || '';
      if (href.charAt(0) !== '#') {
        var resolved = link.href || '';
        if (resolved.indexOf('file:') === 0 && window.ipc && window.ipc.postMessage) {
          event.preventDefault();
          window.ipc.postMessage('open-local-link:' + resolved);
        }
        return;
      }

      event.preventDefault();

      var fragment = href.slice(1);
      if (!fragment) {
        window.scrollTo(0, 0);
        updateHash(fragment);
        return;
      }

      var decoded = decodeFragment(fragment);
      var target = document.getElementById(decoded) || document.getElementById(fragment);
      if (!target) return;

      target.scrollIntoView({ block: 'start' });
      updateHash(fragment);
    });
  }

  window.__setFeatureFlags = setFlags;

  window.__setKatexCss = function(css) {
    if (document.getElementById('katex-css')) return;
    var style = document.createElement('style');
    style.id = 'katex-css';
    style.textContent = css;
    document.head.appendChild(style);
  };

  window.__enhancePreview = function() {
    var root = document.getElementById('preview');
    if (!root) return;
    bindAnchorNavigation();
    bindLightbox();
    idle(function() {
      enhanceTables(root);
      enhanceAlerts(root);
      enhanceMarks(root);
      enhanceMath(root);
      enhanceMermaid(root);
    });
  };
})();
