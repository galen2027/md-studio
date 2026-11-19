(function() {
  window.__mdPreviewInstallSplitSync = function(options) {
    var editor = options.editor, preview = options.preview;
    var active = false, points = [], timer = 0, last_driver = editor;
    var expected_scroll = new WeakMap();
    var mirror = document.createElement('div');
    mirror.style.cssText = 'position:fixed;left:-100000px;top:0;visibility:hidden;white-space:pre-wrap;overflow-wrap:break-word;pointer-events:none;border:0;';
    document.body.appendChild(mirror);
    function interpolate(value, from, to) {
      if (!points.length) return 0;
      var a = points[0], b = points[points.length - 1];
      for (var i = 1; i < points.length; i++) {
        if (points[i][from] > value) { b = points[i]; a = points[i - 1]; break; }
        a = points[i];
      }
      var span = b[from] - a[from];
      return a[to] + (span > 0 ? Math.max(0, Math.min(1, (value - a[from]) / span)) * (b[to] - a[to]) : 0);
    }
    function move(target, top) {
      var next = Math.max(0, Math.min(target.scrollHeight - target.clientHeight, top));
      if (Math.abs(target.scrollTop - next) < 1) return;
      expected_scroll.set(target, next);
      target.scrollTop = next;
    }
    function sync(driver) {
      if (!active || !points.length) return;
      if (driver === editor) move(preview, interpolate(editor.scrollTop, 'editor', 'preview'));
      else move(editor, interpolate(preview.scrollTop, 'preview', 'editor'));
    }
    function rebuild() {
      timer = 0;
      if (!active) return;
      var anchor = points.length ? interpolate(last_driver.scrollTop, last_driver === editor ? 'editor' : 'preview', 'offset') : null;
      var style = getComputedStyle(editor);
      ['fontFamily','fontSize','fontWeight','fontStyle','lineHeight','letterSpacing','wordSpacing','tabSize','paddingTop','paddingRight','paddingBottom','paddingLeft','boxSizing','wordBreak'].forEach(function(key) { mirror.style[key] = style[key]; });
      mirror.style.width = editor.clientWidth + 'px';
      mirror.textContent = editor.value + '\n';
      var text = mirror.firstChild, range = document.createRange();
      var origin = mirror.getBoundingClientRect().top;
      var nodes = Array.from(preview.querySelectorAll('[data-source-start]'));
      points = [{ offset: 0, editor: 0, preview: 0 }];
      nodes.forEach(function(node) {
        var offset = Number(node.getAttribute('data-source-start'));
        if (offset < 0 || offset > editor.value.length) return;
        range.setStart(text, offset); range.setEnd(text, Math.min(text.length, offset + 1));
        var rect = range.getBoundingClientRect();
        var y = node.getBoundingClientRect().top - preview.getBoundingClientRect().top + preview.scrollTop;
        var previous = points[points.length - 1];
        if (offset <= previous.offset || y < previous.preview) return;
        points.push({ offset: offset, editor: rect.top - origin, preview: y });
      });
      points.push({ offset: editor.value.length, editor: mirror.scrollHeight, preview: preview.scrollHeight });
      if (anchor !== null) {
        move(editor, interpolate(anchor, 'offset', 'editor'));
        move(preview, interpolate(anchor, 'offset', 'preview'));
      } else sync(last_driver);
    }
    function refresh() {
      if (timer) clearTimeout(timer);
      timer = setTimeout(rebuild, 60);
    }
    [editor, preview].forEach(function(pane) {
      pane.addEventListener('scroll', function() {
        if (!active) return;
        var expected = expected_scroll.get(pane);
        expected_scroll.delete(pane);
        if (typeof expected === 'number' && Math.abs(pane.scrollTop - expected) < 2) return;
        last_driver = pane;
        sync(pane);
      }, { passive: true });
    });
    function reveal_caret() {
      if (!active || !points.length) return;
      last_driver = editor;
      var y = interpolate(editor.selectionStart, 'offset', 'preview');
      if (y < preview.scrollTop + 24 || y > preview.scrollTop + preview.clientHeight - 48) {
        move(preview, y - preview.clientHeight * 0.25);
      }
    }
    editor.addEventListener('keyup', reveal_caret);
    editor.addEventListener('click', reveal_caret);
    editor.addEventListener('input', function() { refresh(); });
    preview.addEventListener('load', refresh, true);
    new MutationObserver(refresh).observe(preview, { childList: true, subtree: true });
    window.addEventListener('resize', refresh);
    function capture() {
      if (active && points.length) return interpolate(last_driver.scrollTop, last_driver === editor ? 'editor' : 'preview', 'offset');
      var nodes = Array.from(preview.querySelectorAll('[data-source-start]'));
      var header = (parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--chrome-top')) || 0) + 60;
      var chosen = nodes[0];
      nodes.forEach(function(node) { if (node.getBoundingClientRect().top <= header) chosen = node; });
      return chosen ? Number(chosen.getAttribute('data-source-start')) : 0;
    }
    return {
      active: function(value) {
        active = value;
        expected_scroll = new WeakMap();
        if (active) { last_driver = editor; rebuild(); reveal_caret(); }
      },
      refresh: refresh,
      capture: capture,
      seek: function(offset, reveal) {
        if (!active) return;
        last_driver = editor;
        var context = reveal ? 60 : 0;
        move(editor, interpolate(offset, 'offset', 'editor') - context);
        move(preview, interpolate(offset, 'offset', 'preview') - context);
      },
      restore_reading: function(offset) {
        function restore() {
          if (active) return;
          var chosen = null;
          preview.querySelectorAll('[data-source-start]').forEach(function(node) {
            if (Number(node.getAttribute('data-source-start')) <= offset) chosen = node;
          });
          if (chosen) window.scrollTo(0, window.scrollY + chosen.getBoundingClientRect().top - 88);
        }
        requestAnimationFrame(function() { restore(); requestAnimationFrame(restore); });
      },
      // Exposed coordinates make semantic alignment independently verifiable.
      position: function() { return { points: points, driver: last_driver === editor ? 'editor' : 'preview' }; }
    };
  };
})();
