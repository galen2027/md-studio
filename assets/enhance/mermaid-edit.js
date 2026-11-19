// Conservative source patches, checked against the bundled Mermaid parser.
(function() {
  var sequence = 0;
  function fail(message) { throw new Error(message); }
  function statements(source) {
    var result = [], start = 0, stack = [], quote = false, comment = false;
    for (var i = 0; i < source.length; i++) {
      var ch = source[i];
      if (comment) { if (ch !== '\n') continue; comment = false; }
      else if (!quote && !stack.length && source.slice(i, i + 2) === '%%') { comment = true; continue; }
      else if (ch === '"' && source[i - 1] !== '\\') { quote = !quote; }
      else if (!quote) {
        if ('[({'.includes(ch)) stack.push({ '[': ']', '(': ')', '{': '}' }[ch]);
        else if (ch === stack[stack.length - 1]) stack.pop();
      }
      if ((ch === '\n' || ch === ';') && !quote && !stack.length) {
        result.push({ start: start, end: i, text: source.slice(start, i) }); start = i + 1;
      }
    }
    result.push({ start: start, end: source.length, text: source.slice(start) });
    return result;
  }
  var shapes = [['(((', ')))'], ['[[', ']]'], ['([', '])'], ['((', '))'], ['{{', '}}'], ['[(', ')]'], ['[/', '/]'], ['[\\', '\\]'], ['[', ']'], ['(', ')'], ['{', '}'], ['>', ']']];
  function chain(text) {
    var pos = 0, groups = [], links = [];
    function space() { while (/\s/.test(text[pos] || '') && pos < text.length) pos++; }
    function node() {
      space();
      var start = pos, id = '';
      while (pos < text.length && /[\w\u0080-\uffff-]/.test(text[pos])) {
        if (/^(?:--|-\.|==)/.test(text.slice(pos))) break;
        id += text[pos++];
      }
      if (!id) fail('无法定位节点，保留原源码。');
      var id_end = pos; space();
      var label_start = null, label_end = null, shape = shapes.find(function(pair) { return text.slice(pos).startsWith(pair[0]); });
      if (shape) {
        pos += shape[0].length; label_start = pos;
        var quote = false;
        while (pos < text.length) {
          if (text[pos] === '"' && text[pos - 1] !== '\\') quote = !quote;
          if (!quote && text.slice(pos).startsWith(shape[1])) break;
          pos++;
        }
        if (pos === text.length) fail('节点形状无法安全定位，请使用源码编辑。');
        label_end = pos; pos += shape[1].length;
      } else pos = id_end;
      if (text.slice(pos).startsWith(':::')) {
        var cls = text.slice(pos).match(/^:::[\w-]+/);
        if (!cls) fail('不支持的节点样式。');
        pos += cls[0].length;
      }
      return { id: id, start: start, end: pos, id_end: id_end, label_start: label_start, label_end: label_end, raw: text.slice(start, pos) };
    }
    function group() {
      var nodes = [node()]; space();
      while (text[pos] === '&') { pos++; nodes.push(node()); space(); }
      return nodes;
    }
    space();
    if (!text.slice(pos) || /^(?:%%|flowchart\b|graph\b|subgraph\b|end\b|direction\b|classDef\b|class\b|style\b|click\b|linkStyle\b)/.test(text.slice(pos))) return null;
    try {
      groups.push(group());
      while (pos < text.length && !text.slice(pos).startsWith('%%')) {
        var start = pos;
        var arrow = text.slice(pos).match(/^(?:--|==|-\.)\s+[^|\n][\s\S]*?(?:-->|---|==>|-\.->)/) ||
          text.slice(pos).match(/^(?:[ox])?<?[-=.]{2,}[>ox]?/);
        if (!arrow) return null;
        pos += arrow[0].length; space();
        if (text[pos] === '|') {
          var end = text.indexOf('|', pos + 1);
          if (end < 0) return null;
          pos = end + 1; space();
        }
        links.push(text.slice(start, pos));
        groups.push(group());
      }
      return { groups: groups, links: links, comment: text.slice(pos) };
    } catch (_) { return null; }
  }
  function patches(source, changes) {
    changes.sort(function(a, b) { return b.start - a.start; }).forEach(function(change) {
      source = source.slice(0, change.start) + change.value + source.slice(change.end);
    });
    return source;
  }
  function encode_label(text) {
    var entities = { '#': '#35;', '"': '#quot;', '&': '#38;', '<': '#60;', '>': '#62;', '[': '#91;', ']': '#93;', '`': '#96;' };
    return '"' + text.replace(/[#"&<>\[\]`]/g, function(ch) { return entities[ch]; }).replace(/\r\n?|\n/g, '<br/>') + '"';
  }
  async function model(source) {
    if (!/^\s*(?:%%[^\n]*\n\s*)*(?:flowchart|graph)\b/.test(source)) fail('本版支持 flowchart / graph 流程图。此类图请使用源码编辑。');
    var diagram = await window.mermaid.mermaidAPI.getDiagramFromText(source);
    if (!diagram.db.getVertices || !diagram.db.getEdges) fail('当前图类型不支持节点编辑。');
    var vertices = diagram.db.getVertices();
    var nodes = vertices instanceof Map ? Object.fromEntries(vertices) : vertices;
    // Mermaid databases may be shared across parses; snapshot before awaiting.
    return JSON.parse(JSON.stringify({ nodes: nodes, edges: diagram.db.getEdges() }));
  }
  function edge_keys(edges) {
    return edges.map(function(edge) {
      return JSON.stringify([edge.start, edge.end, edge.text || '', edge.type, edge.stroke, edge.length, edge.style || [], edge.classes || []]);
    }).sort();
  }
  async function change(source, id, operation, text) {
    var before = await model(source);
    if (!Object.hasOwn(before.nodes, id)) fail('节点已变化，请重新打开流程图。');
    var edits = [], found = false;
    if (operation === 'delete' && /^\s*linkStyle\s+\d/m.test(source)) fail('该图使用连线序号样式，请使用源码删除节点，避免样式错位。文字编辑仍可使用。');
    statements(source).forEach(function(statement) {
      var parsed = chain(statement.text);
      if (!parsed) {
        if (operation !== 'delete') return;
        var style = statement.text.match(/^(\s*)(style|click)\s+(\S+)/);
        if (style && style[3] === id) edits.push({ start: statement.start, end: statement.end, value: '' });
        var cls = statement.text.match(/^(\s*class\s+)([^\s]+)(\s+.+)$/);
        if (cls && cls[2].split(',').includes(id)) {
          var remaining = cls[2].split(',').filter(function(value) { return value !== id; });
          edits.push({ start: statement.start, end: statement.end, value: remaining.length ? cls[1] + remaining.join(',') + cls[3] : '' });
        }
        return;
      }
      var nodes = parsed.groups.flat();
      if (!nodes.some(function(node) { return node.id === id; })) return;
      if (operation === 'edit') {
        nodes.filter(function(node) { return node.id === id && node.label_start !== null; }).forEach(function(node) {
          found = true;
          edits.push({ start: statement.start + node.label_start, end: statement.start + node.label_end, value: encode_label(text) });
        });
      } else {
        found = true;
        var groups = parsed.groups.map(function(group) { return group.filter(function(node) { return node.id !== id; }); });
        var chunks = [], used = new Set();
        parsed.links.forEach(function(link, index) {
          if (!groups[index].length || !groups[index + 1].length) return;
          chunks.push(groups[index].map(function(node) { return node.raw; }).join(' & ') + ' ' + link + groups[index + 1].map(function(node) { return node.raw; }).join(' & '));
          groups[index].concat(groups[index + 1]).forEach(function(node) { used.add(node); });
        });
        groups.flat().forEach(function(node) { if (!used.has(node)) chunks.push(node.raw); });
        var indent = statement.text.match(/^\s*/)[0];
        edits.push({ start: statement.start, end: statement.end, value: chunks.map(function(chunk) { return indent + chunk; }).join('\n') + (parsed.comment ? '\n' + indent + parsed.comment : '') });
      }
    });
    if (operation === 'edit' && !found) {
      statements(source).some(function(statement) {
        var parsed = chain(statement.text), node = parsed && parsed.groups.flat().find(function(node) { return node.id === id; });
        if (!node) return false;
        edits.push({ start: statement.start + node.id_end, end: statement.start + node.id_end, value: '[' + encode_label(text) + ']' });
        found = true; return true;
      });
    }
    if (!found) fail('此节点使用的扩展语法暂不支持安全修改，请使用源码编辑。');
    var next = patches(source, edits), after = await model(next);
    var expected_ids = Object.keys(before.nodes).filter(function(key) { return operation !== 'delete' || key !== id; }).sort();
    if (JSON.stringify(expected_ids) !== JSON.stringify(Object.keys(after.nodes).sort())) fail('修改会影响其他节点，已取消，请使用源码编辑。');
    expected_ids.forEach(function(key) {
      if (operation === 'edit' && key === id) {
        // Mermaid leaves the type undefined for a bare ID; it renders as the
        // same default rectangle used when we add its first explicit label.
        if ((before.nodes[key].type || 'square') !== (after.nodes[key].type || 'square')) fail('节点形状发生意外变化，已取消。');
      } else {
        var left = Object.assign({}, before.nodes[key]), right = Object.assign({}, after.nodes[key]);
        delete left.domId; delete right.domId;
        if (JSON.stringify(left) !== JSON.stringify(right)) fail('其他节点属性发生变化，已取消。');
      }
    });
    var expected_edges = before.edges.filter(function(edge) { return operation !== 'delete' || (edge.start !== id && edge.end !== id); });
    if (JSON.stringify(edge_keys(expected_edges)) !== JSON.stringify(edge_keys(after.edges))) fail('连线发生意外变化，已取消，请使用源码编辑。');
    return { source: next, model: after };
  }
  function fences(raw) {
    var lines = raw.match(/[^\n]*\n|[^\n]+$/g) || [], result = [], offset = 0, open = null;
    lines.forEach(function(line) {
      var match;
      if (!open && (match = line.match(/^ {0,3}(`{3,}|~{3,})([^\n]*)\r?\n$/))) {
        open = { fence: match[1], start: offset + line.length, mermaid: match[2].trim().toLowerCase() === 'mermaid' };
      } else if (open && new RegExp('^ {0,3}' + open.fence[0] + '{' + open.fence.length + ',}\\s*$').test(line)) {
        if (open.mermaid) result.push({ start: open.start, end: offset, source: raw.slice(open.start, offset) }); open = null;
      }
      offset += line.length;
    });
    return result;
  }
  window.__mdPreviewMermaidTools = { model: model, change: change, fences: fences };

  window.__mdPreviewInstallDiagramEditor = function(options) {
    var current = null, panel = null, toggle = null, binding = null, snapshot = null, history = [], busy = false, revision = 0;
    var zh = options.zh;
    function message(text) { panel.querySelector('[data-diagram-status]').textContent = text; }
    function node_id(node) {
      var id = node.getAttribute('data-id');
      if (Object.hasOwn(snapshot.nodes, id)) return id;
      return Object.keys(snapshot.nodes).find(function(key) {
        var marker = 'flowchart-' + key + '-', index = node.id.lastIndexOf(marker);
        return node.id === snapshot.nodes[key].domId || (index >= 0 && (index === 0 || node.id[index - 1] === '-') && /^\d+$/.test(node.id.slice(index + marker.length)));
      });
    }
    function select_node(id) {
      if (!snapshot || !Object.hasOwn(snapshot.nodes, id)) return;
      panel.querySelector('select').value = id;
      var value = snapshot.nodes[id].text || id;
      var holder = new DOMParser().parseFromString(value.replace(/#(\d+|[a-zA-Z]+);/g, '&$1;').replace(/<br\s*\/?\s*>/gi, '\n'), 'text/html');
      panel.querySelector('textarea').value = holder.body.textContent;
      panel.querySelector('[data-diagram-confirm]').hidden = true;
      current.lightbox.querySelectorAll('g.node').forEach(function(node) { node.classList.toggle('mdp-node-selected', node_id(node) === id); });
    }
    function populate(id) {
      panel.querySelector('[data-diagram-confirm]').hidden = true;
      var select = panel.querySelector('select'); select.textContent = '';
      Object.keys(snapshot.nodes).forEach(function(key) {
        var option = document.createElement('option'); option.value = key; option.textContent = key; select.appendChild(option);
      });
      panel.querySelector('[data-diagram-undo]').disabled = !history.length;
      var chosen = Object.hasOwn(snapshot.nodes, id) ? id : Object.keys(snapshot.nodes)[0];
      panel.querySelectorAll('[data-diagram-save],[data-diagram-delete],textarea,select').forEach(function(element) { element.disabled = !chosen; });
      if (chosen) select_node(chosen); else panel.querySelector('textarea').value = '';
    }
    async function initialize() {
      var epoch = ++revision;
      panel.hidden = false; message(zh ? '正在读取节点…' : 'Loading nodes…');
      window.__mdPreviewFitDiagram();
      panel.querySelectorAll('select,textarea,[data-diagram-save],[data-diagram-delete],[data-diagram-undo]').forEach(function(element) { element.disabled = true; });
      try {
        var next_binding = options.bind(current.container);
        var next_snapshot = await model(next_binding.source);
        if (epoch !== revision) return;
        binding = next_binding; snapshot = next_snapshot; history = [];
        populate(); message(zh ? '在图上点击节点，修改文字后点“应用”；Ctrl+S 保存到原文件。' : 'Select a node in the diagram, edit its text and Apply. Ctrl+S saves the file.');
      } catch (error) { if (epoch === revision) { snapshot = null; message(error.message); } }
    }
    async function apply(operation) {
      if (busy || !binding || !snapshot) return;
      busy = true; var epoch = revision;
      panel.querySelectorAll('button').forEach(function(button) { button.disabled = true; });
      try {
        var id = panel.querySelector('select').value;
        var previous = binding.source;
        var result = operation === 'undo' ? { source: history[history.length - 1], model: await model(history[history.length - 1]) } :
          await change(binding.source, id, operation, panel.querySelector('textarea').value);
        // Render before modifying the document; invalid input never reaches disk.
        var rendered = await window.mermaid.render('mdp-edited-' + (++sequence), result.source);
        if (epoch !== revision) fail(zh ? '文档已切换，操作已取消。' : 'Document changed. Action cancelled.');
        binding = await options.apply(binding, result.source);
        if (operation === 'undo') history.pop(); else history.push(previous);
        snapshot = result.model;
        var holder = document.createElement('div'); holder.innerHTML = rendered.svg || rendered;
        window.__mdPreviewRefreshDiagram(holder);
        populate(id); message(zh ? '已应用，自动保存 / Ctrl+S。修改前的原文件已备份。' : 'Applied. Autosave / Ctrl+S. Original backed up.');
      } catch (error) { message(error.message); }
      finally {
        busy = false;
        panel.querySelectorAll('button').forEach(function(button) { button.disabled = false; });
        panel.querySelector('[data-diagram-undo]').disabled = !history.length;
        panel.querySelectorAll('[data-diagram-save],[data-diagram-delete],[data-diagram-confirm]').forEach(function(button) { button.disabled = !snapshot || !Object.keys(snapshot.nodes).length; });
      }
    }
    document.addEventListener('mdpreview-diagram-open', function(event) {
      revision++; current = event.detail; binding = snapshot = null; history = [];
      if (!panel) {
        toggle = document.createElement('button'); toggle.type = 'button';
        toggle.textContent = zh ? '编辑节点' : 'Edit nodes'; toggle.setAttribute('data-diagram-edit', '');
        current.lightbox.querySelector('[data-lb="png"]').before(toggle);
        panel = document.createElement('section'); panel.className = 'mdp-diagram-editor'; panel.hidden = true;
        panel.setAttribute('aria-label', zh ? '流程图节点编辑' : 'Flowchart node editor');
        panel.innerHTML = '<label>' + (zh ? '节点' : 'Node') + '<select></select></label><label>' + (zh ? '节点文字' : 'Text') +
          '<textarea rows="4"></textarea></label><div class="mdp-diagram-actions"><button data-diagram-save>' + (zh ? '应用文字' : 'Apply text') +
          '</button><button data-diagram-delete>' + (zh ? '删除节点及连线' : 'Delete node and edges') + '</button><button data-diagram-undo>' +
          (zh ? '撤销上次图修改' : 'Undo diagram change') + '</button><button data-diagram-hide>' + (zh ? '收起' : 'Hide') +
          '</button></div><button data-diagram-confirm hidden>' + (zh ? '确认删除该节点及关联连线' : 'Confirm deletion') + '</button><p data-diagram-status role="status"></p>';
        current.lightbox.appendChild(panel);
        toggle.addEventListener('click', function() { if (panel.hidden) initialize(); else { panel.hidden = true; window.__mdPreviewFitDiagram(); } });
        panel.querySelector('select').addEventListener('change', function() { select_node(this.value); });
        panel.querySelector('[data-diagram-save]').addEventListener('click', function() { apply('edit'); });
        panel.querySelector('[data-diagram-delete]').addEventListener('click', function() { panel.querySelector('[data-diagram-confirm]').hidden = false; });
        panel.querySelector('[data-diagram-confirm]').addEventListener('click', function() { apply('delete'); });
        panel.querySelector('[data-diagram-undo]').addEventListener('click', function() { apply('undo'); });
        panel.querySelector('[data-diagram-hide]').addEventListener('click', function() { panel.hidden = true; window.__mdPreviewFitDiagram(); });
        current.lightbox.addEventListener('click', function(click) {
          if (panel.hidden || !snapshot) return;
          var node = click.target.closest && click.target.closest('g.node');
          if (!node) return;
          var id = node_id(node);
          if (id) select_node(id);
        });
        panel.addEventListener('keydown', function(key) { if (!(key.ctrlKey || key.metaKey)) key.stopPropagation(); });
      }
      panel.hidden = true; toggle.disabled = false;
    });
  };
})();
