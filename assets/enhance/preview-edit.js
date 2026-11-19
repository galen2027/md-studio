// Structured preview editing backed by pinned, offline Vditor.
(function() {
  var asset_loading = null;
  var secured_mermaid = null;
  function secure_mermaid() {
    var mermaid = window.mermaid;
    if (!mermaid || mermaid === secured_mermaid) return;
    var initialize = mermaid.initialize.bind(mermaid);
    mermaid.initialize = function(config) {
      return initialize(Object.assign({}, config, { securityLevel: 'strict' }));
    };
    secured_mermaid = mermaid;
  }
  document.addEventListener('load', function(event) {
    if (event.target.tagName === 'SCRIPT' && /\/mermaid\.min\.js(?:\?|$)/.test(event.target.src)) secure_mermaid();
  }, true);
  function load_assets(base) {
    if (asset_loading) return asset_loading;
    asset_loading = new Promise(function(resolve, reject) {
      var css = document.createElement('link');
      css.rel = 'stylesheet'; css.href = base + '/dist/index.css';
      document.head.appendChild(css);
      var script = document.createElement('script');
      script.src = base + '/dist/index.min.js';
      script.onload = resolve;
      script.onerror = function() { reject(new Error('Editor resource could not be loaded')); };
      document.head.appendChild(script);
    }).catch(function(error) { asset_loading = null; throw error; });
    return asset_loading;
  }
  function split_metadata(raw) {
    var match = raw.match(/^(?:\uFEFF)?---\n[\s\S]*?\n(?:---|\.\.\.)[ \t]*(?:\n|$)/);
    if (match) return [match[0], raw.slice(match[0].length)];
    return raw.charCodeAt(0) === 0xFEFF ? [raw.charAt(0), raw.slice(1)] : ['', raw];
  }

  window.__mdPreviewInstallPreviewEdit = function(options) {
    var root = options.root, textarea = options.textarea, button = options.button;
    var host = options.host, hint = options.hint;
    var engine = null, enabled = false, composing = false, generation = 0;
    var prefix = '', original = '', initial_serialized = '', last_serialized = '', expected = '';
    var observer = null, pending_sync = 0, image_map = new Map(), image_sources = new Map();
    var saved_selection = null;
    var zh = options.zh;

    function set_hint(text, error) {
      hint.textContent = text;
      hint.classList.toggle('error', !!error);
    }
    function update_images(html) {
      var document_root = html ? new DOMParser().parseFromString(html, 'text/html') : root;
      document_root.querySelectorAll('img[data-mdp-source]').forEach(function(image) {
        var source = image.getAttribute('data-mdp-source'), url = image.getAttribute('src');
        image_map.set(source, url);
        image_sources.set(url, source);
      });
      decorate();
    }
    function edit_root() { return engine && engine.vditor.wysiwyg.element; }
    function decorate() {
      var element = edit_root();
      if (!element) return;
      // A document <base> makes SVG fragment <use> links resolve to the MD
      // directory. Inline the pinned sprite paths instead of fetching a file.
      host.querySelectorAll('svg use').forEach(function(use) {
        var reference = use.getAttribute('href') || use.getAttribute('xlink:href') || '';
        var symbol = reference.charAt(0) === '#' && document.getElementById(reference.slice(1));
        if (symbol) {
          var svg = use.parentElement;
          svg.setAttribute('viewBox', symbol.getAttribute('viewBox') || '0 0 1024 1024');
          svg.innerHTML = symbol.innerHTML;
        }
      });
      element.querySelectorAll('img').forEach(function(image) {
        var current = image.getAttribute('src');
        var source = image.getAttribute('data-mdp-source') || image_sources.get(current) || current;
        var url = image_map.get(source);
        if (url && current !== url) image.setAttribute('src', url);
        if (url && image.getAttribute('data-mdp-source') !== source) image.setAttribute('data-mdp-source', source);
      });
      element.querySelectorAll('.vditor-wysiwyg__preview .language-mermaid').forEach(function(panel) {
        if (!panel.classList.contains('mdp-mermaid')) panel.classList.add('mdp-mermaid');
        var block = panel.closest('.vditor-wysiwyg__block');
        var code = block && block.querySelector('pre:not(.vditor-wysiwyg__preview) > code');
        if (code) panel.dataset.mdpDiagramSource = code.textContent.replace(/\u200b/g, '');
      });
      // Vditor exposes raw inline HTML as code. Render the common table break
      // visually while the structured serializer preserves its meaning.
      element.querySelectorAll('code[data-type="html-inline"]').forEach(function(code) {
        if (/^<br\s*\/?\s*>$/i.test(code.textContent.replace(/\u200b/g, '').trim())) {
          var br = document.createElement('br');
          br.setAttribute('data-mdp-br', '1');
          code.replaceWith(br);
        }
      });
    }
    function serialize() {
      var clone = edit_root().cloneNode(true);
      clone.querySelectorAll('img').forEach(function(image) {
        var source = image.getAttribute('data-mdp-source') || image_sources.get(image.getAttribute('src'));
        if (source) image.setAttribute('src', source);
        image.removeAttribute('data-mdp-source');
      });
      return engine.vditor.lute.VditorDOM2Md(clone.innerHTML).replace(/\u00a0/g, ' ');
    }
    function flush(force) {
      if (pending_sync) { clearTimeout(pending_sync); pending_sync = 0; }
      if (!enabled || !engine || (composing && !force)) return;
      if (textarea.value !== expected) {
        reset();
        set_hint(zh ? '文档已变化，请重新开启预览编辑。' : 'Document changed. Reopen preview editing.', true);
        document.body.classList.add('rich-edit-error');
        return;
      }
      var markdown = serialize();
      if (markdown === last_serialized) return;
      last_serialized = markdown;
      var next = markdown === initial_serialized ? original : prefix + markdown;
      if (next === textarea.value) return;
      expected = next;
      textarea.value = next;
      options.on_input({ isComposing: false });
    }
    function schedule_sync() {
      if (!enabled || composing) return;
      if (pending_sync) clearTimeout(pending_sync);
      pending_sync = setTimeout(function() { pending_sync = 0; flush(); }, 80);
    }
    function reset() {
      generation++;
      saved_selection = null;
      if (pending_sync) { clearTimeout(pending_sync); pending_sync = 0; }
      if (observer) { observer.disconnect(); observer = null; }
      if (composing) { composing = false; options.on_composition(false); }
      enabled = false;
      if (engine && engine.vditor && engine.vditor.wysiwyg) engine.destroy();
      engine = null;
      var sprite = document.getElementById('vditor-icon-headings');
      if (sprite && sprite.closest('svg')) sprite.closest('svg').remove();
      image_map.clear(); image_sources.clear();
      host.textContent = '';
      document.body.classList.remove('preview-editing', 'rich-edit-loading');
      if (button) { button.disabled = false; button.classList.remove('on'); button.setAttribute('aria-pressed', 'false'); }
    }
    function disable() { flush(); reset(); }
    async function enable() {
      options.on_enter();
      var epoch = ++generation, raw = textarea.value;
      document.body.classList.remove('rich-edit-error');
      document.body.classList.add('rich-edit-loading');
      button.disabled = true;
      set_hint(zh ? '正在备份原文件并加载离线编辑器…' : 'Backing up the original and loading the offline editor…');
      try {
        await options.prepare();
        if (epoch !== generation) return;
        await load_assets(options.asset_base);
        if (epoch !== generation || textarea.value !== raw) return;
        original = expected = raw;
        var parts = split_metadata(raw);
        prefix = parts[0];
        update_images();
        host.textContent = '';
        if (prefix) {
          var metadata = document.createElement('pre');
          metadata.className = 'rich-metadata';
          metadata.textContent = prefix.trimEnd();
          host.appendChild(metadata);
        }
        var container = document.createElement('div');
        host.appendChild(container);
        var dark = window.matchMedia('(prefers-color-scheme: dark)').matches;
        secure_mermaid();
        await new Promise(function(resolve, reject) {
          var timeout = setTimeout(function() { reject(new Error(zh ? '编辑器加载超时，请重试。' : 'Editor loading timed out. Please retry.')); }, 15000);
          engine = new Vditor(container, {
            value: parts[1], mode: 'wysiwyg', cdn: options.asset_base,
            lang: zh ? 'zh_CN' : 'en_US', cache: { enable: false },
            toolbar: ['headings', 'bold', 'italic', 'strike', 'link', '|', 'list', 'ordered-list', 'check', 'quote', 'inline-code', 'code', 'table', '|', 'undo', 'redo'],
            toolbarConfig: { pin: true }, undoDelay: 100, minHeight: 300,
            theme: dark ? 'dark' : 'classic',
            hint: { parse: false, emoji: {} },
            preview: { actions: [], hljs: { enable: false, style: dark ? 'github-dark' : 'github' },
              theme: { current: dark ? 'dark' : 'light', path: options.asset_base + '/dist/css/content-theme' },
              markdown: { autoSpace: false, fixTermTypo: false, mark: true, codeBlockPreview: true, mathBlockPreview: true },
              render: { media: { enable: false } } },
            link: { isOpen: false }, image: { isPreview: false },
            input: schedule_sync,
            after: function() { clearTimeout(timeout); resolve(); }
          });
        });
        if (epoch !== generation) return;
        decorate();
        initial_serialized = last_serialized = serialize();
        enabled = true;
        var element = edit_root();
        element.setAttribute('aria-label', zh ? '结构化 Markdown 编辑器' : 'Structured Markdown editor');
        element.addEventListener('input', function() { options.on_dirty(); schedule_sync(); });
        element.addEventListener('compositionstart', function() {
          options.on_dirty();
          composing = true;
          if (pending_sync) { clearTimeout(pending_sync); pending_sync = 0; }
          options.on_composition(true);
        });
        element.addEventListener('compositionend', function() {
          composing = false; options.on_composition(false); schedule_sync();
        });
        observer = new MutationObserver(decorate);
        observer.observe(host, { childList: true, subtree: true });
        document.body.classList.remove('rich-edit-loading');
        document.body.classList.add('preview-editing');
        button.classList.add('on'); button.setAttribute('aria-pressed', 'true');
        set_hint(zh ? '所见即所得编辑 · 标题、段落、列表、表格均可修改；自动保存 / Ctrl+S。原文件已备份，排版源码会规范化。' :
          'Edit headings, paragraphs, lists and tables. Autosave / Ctrl+S. Original backed up; Markdown formatting may normalize.');
        if (options.on_ready) options.on_ready();
      } catch (error) {
        if (epoch !== generation) return;
        reset();
        document.body.classList.add('rich-edit-error');
        set_hint((zh ? '无法开启预览编辑，原文未改动：' : 'Could not open preview editor: ') + error.message, true);
      } finally {
        if (epoch === generation) button.disabled = false;
      }
    }
    button.addEventListener('click', function() {
      if (document.body.classList.contains('empty') || document.body.classList.contains('missing')) return;
      if (enabled) { disable(); options.on_exit(); } else enable();
    });
    host.addEventListener('pointerdown', function(event) {
      if (enabled && event.target.closest('.vditor-toolbar button, .vditor-panel button')) options.on_dirty();
    });
    // Do not let browser navigation take an unfinished edit away.
    host.addEventListener('click', function(event) {
      var action = event.target.closest && event.target.closest('button[data-type]');
      if (enabled && action) {
        var commands = { bold: 'bold', italic: 'italic', strike: 'strikeThrough' };
        var command = commands[action.getAttribute('data-type')];
        var selection = window.getSelection();
        if (command && selection.rangeCount && edit_root().contains(selection.getRangeAt(0).commonAncestorContainer)) {
          // Vditor delays toolbar highlighting. A fast move from a bold cell
          // to a plain paragraph can otherwise execute the opposite action.
          action.classList.toggle('vditor-menu--current', document.queryCommandState(command));
        }
      }
      var diagram = event.target.closest && event.target.closest('.mdp-mermaid');
      if (diagram && diagram.querySelector('svg') && window.__mdPreviewOpenDiagram) {
        event.preventDefault(); event.stopPropagation();
        window.__mdPreviewOpenDiagram(diagram); return;
      }
      if (event.target.closest && event.target.closest('a')) event.preventDefault();
    }, true);
    return {
      mount: function(raw, html) { update_images(html); },
      update_images: update_images,
      active: function() { return enabled; },
      root: edit_root,
      flush: flush,
      replace_markdown: function(raw) {
        if (!enabled || composing) throw new Error(zh ? '请先完成当前输入。' : 'Finish the current composition first.');
        var parts = split_metadata(raw);
        prefix = parts[0];
        engine.setValue(parts[1]);
        decorate();
        original = expected = raw;
        initial_serialized = last_serialized = serialize();
        textarea.value = raw;
        options.on_input({ isComposing: false });
      },
      disable: disable,
      reset: reset,
      focus: function() { if (engine) engine.focus(); },
      remember_selection: function() {
        var selection = window.getSelection();
        if (engine && selection.rangeCount && edit_root().contains(selection.getRangeAt(0).commonAncestorContainer)) {
          saved_selection = selection.getRangeAt(0).cloneRange();
        }
      },
      restore_selection: function() {
        if (!engine || !saved_selection || !edit_root().contains(saved_selection.startContainer)) return;
        engine.focus();
        var selection = window.getSelection();
        selection.removeAllRanges(); selection.addRange(saved_selection);
        saved_selection = null;
      }
    };
  };
})();
