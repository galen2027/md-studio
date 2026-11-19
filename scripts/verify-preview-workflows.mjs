import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {mkdtemp,readFile,writeFile} from 'node:fs/promises';
import {resolve,dirname} from 'node:path';
import {fileURLToPath} from 'node:url';
import {spawn} from 'node:child_process';
import {createServer} from 'node:net';
const {chromium}=createRequire(import.meta.url)('playwright');
const root=resolve(dirname(fileURLToPath(import.meta.url)),'..');
const realSources=await Promise.all(process.argv.slice(2).map(path=>readFile(path,'utf8')));
const temp=await mkdtemp(resolve(root,'target/preview-workflows-'));
const path=resolve(temp,'工作流.md'), warnings=resolve(temp,'warnings.txt');
const graph='flowchart LR\n  A[开始] -->|下一步| B{检查}\n  B -->|是| C[成功]\n  B -->|否| D[重试]\n  C --> E[结束]\n  classDef blue fill:#eef\n  class A,C blue\n';
const document='# 工作流验证\n\n'+Array.from({length:85},(_,i)=>`## 段落 ${i}\n\n${'这是内容较长的中文段落，用于验证源码与预览在换行、表格和大图下仍对应同一位置。'.repeat(3)}\n\n${i%9===0?'| 字段 | 说明 |\n|---|---|\n'+Array.from({length:8},(_,j)=>`| 字段${j} | ${'详细说明'.repeat(j+2)} |`).join('\n')+'\n\n':''}${i===10?'\n\x60\x60\x60mermaid\n'+graph+'\x60\x60\x60\n\n':''}`).join('\n')+'\n\x60\x60\x60mermaid\n'+graph+'\x60\x60\x60\n';
await writeFile(path,document);
const server=createServer(); await new Promise(done=>server.listen(0,'127.0.0.1',done));
const port=server.address().port; await new Promise(done=>server.close(done));
const child=spawn(resolve(root,'target/debug/md-preview.exe'),[path],{cwd:root,windowsHide:true,stdio:'ignore',env:{...process.env,
  MD_PREVIEW_CONFIG_DIR:resolve(temp,'config'),MD_PREVIEW_TEST_SKIP_REGISTRATION:'1',MD_PREVIEW_TEST_WARNING_LOG:warnings,
  WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${port}`}});
let browser;
async function savedContains(value) {
  for(let i=0;i<70;i++){if((await readFile(path,'utf8')).includes(value))return;await new Promise(done=>setTimeout(done,100));}
  throw new Error('File not saved: '+value);
}
try {
  for(let i=0;i<70;i++){try{browser=await chromium.connectOverCDP('http://127.0.0.1:'+port);break;}catch{await new Promise(done=>setTimeout(done,100));}}
  assert.ok(browser,'WebView2 started');
  const page=browser.contexts()[0].pages()[0]; page.setDefaultTimeout(10000);
  await page.waitForSelector('#preview h1');
  const errors=[];page.on('pageerror',error=>{errors.push(error.message);console.log(error.stack);});
  await page.waitForSelector('#preview .mdp-mermaid svg');
  const results=await page.evaluate(async(realSources)=>{
    const tools=window.__mdPreviewMermaidTools;
    const cases=[
      'flowchart LR\n A[一] -->|OK| B{二}\n B --> C[三]\n C --> D[四]',
      'graph TD;A[一]-->B[二]-->C[三]-->D[四];C-->E[五]',
      'flowchart LR\n subgraph S[子图]\n A([一]) --> B((二)) --> C{{三}}\n end\n C --> D[(四)]\n style C fill:#eee\n classDef green fill:#efe\n class B,C green',
      'flowchart LR\n A & B --> C & D\n B -.-> E[五]\n C ==>|YES| E',
      'flowchart LR\n A -- 文字 --> B\n B --> C\n A --> D',
      'flowchart LR\n A[一] --> B[二]\n B[二] --> C[三]\n %% B[注释不应修改]\n A --> C',
    ];
    for(const source of cases){
      let edited;
      try { edited=await tools.change(source,'B','edit','新文字 "引号" [括号] 🙂\n第二行'); }
      catch(error){throw new Error(source+'\n'+error.message);}
      if(!edited.source.includes('#quot;'))throw new Error('Quote not encoded');
      await mermaid.render('test-'+Math.random().toString(36).slice(2),edited.source);
      const deleted=await tools.change(source,'B','delete');
      if(Object.hasOwn(deleted.model.nodes,'B')||deleted.model.edges.some(edge=>edge.start==='B'||edge.end==='B'))throw new Error('Dangling node');
    }
    let refused=false;
    try{await tools.change('flowchart LR\n A --> B --> C\n linkStyle 1 stroke:red','B','delete');}catch{refused=true;}
    if(!refused)throw new Error('Indexed styles must not be reassigned');
    const fenced='````text\n```mermaid\nnot a diagram\n```\n````\n~~~mermaid\ngraph TD\n A-->B\n~~~\n';
    if(tools.fences(fenced).length!==1)throw new Error('Nested fence selection');
    const real={diagrams:0,edited:0,deleted:0,refused:[]};
    for(const source of realSources){
      const charts=tools.fences(source).filter(block=>/^\s*(flowchart|graph)\b/.test(block.source)).slice(0,3);
      for(const block of charts){
        real.diagrams++;
        const model=await tools.model(block.source),ids=Object.keys(model.nodes);
        if(!ids.length)continue;
        try{await tools.change(block.source,ids[0],'edit','实测节点 中文 "引号"');real.edited++;}catch(error){real.refused.push(error.message);}
        try{await tools.change(block.source,ids.at(-1),'delete');real.deleted++;}catch(error){real.refused.push(error.message);}
      }
    }
    return {cases:cases.length,real};
  },realSources);
  console.log('[preview-workflows] parser-backed edit/delete cases '+results.cases+' passed; real samples '+JSON.stringify(results.real));
  await page.locator('#btn-toggle').evaluate(button=>button.click());
  await page.locator('#editor').press('Control+Home');await page.keyboard.insertText('自身保存标记\n');await page.keyboard.press('Control+s');
  await savedContains('自身保存标记'); await page.waitForTimeout(1200);
  const saved=await readFile(path,'utf8');
  await page.locator('#editor').press('Control+Home');await page.keyboard.insertText('待保存本地输入\n');
  await writeFile(path,saved); // Same disk contents after the old 500 ms window.
  await page.waitForTimeout(1100);
  assert.ok((await page.locator('#editor').inputValue()).includes('待保存本地输入'));
  assert.equal(await readFile(warnings,'utf8').catch(error=>error.code==='ENOENT'?'':Promise.reject(error)),'');
  await page.keyboard.insertText('外部冲突本地输入\n');
  await writeFile(path,'# 真正的外部版本\n');
  for(let i=0;i<50;i++){if((await readFile(warnings,'utf8').catch(()=>'')))break;await page.waitForTimeout(50);}
  assert.ok((await readFile(warnings,'utf8')).includes('磁盘内容'));
  await page.waitForTimeout(900);
  assert.equal(await readFile(path,'utf8'),'# 真正的外部版本\n');
  assert.ok((await page.locator('#editor').inputValue()).includes('外部冲突本地输入'));
  await page.keyboard.press('Control+s');await savedContains('外部冲突本地输入');
  console.log('[preview-workflows] delayed identical writes ignored; true external conflict preserved and explicit save recovered');
  await page.locator('#btn-split').evaluate(button=>button.click());
  await page.waitForTimeout(250);
  const split=await page.evaluate(()=>{
    const preview=document.querySelector('#preview'),editor=document.querySelector('#editor');
    return {preview:preview.clientHeight,editor:editor.clientHeight,height:innerHeight,overflow:document.documentElement.scrollWidth>innerWidth};
  });
  assert.ok(split.preview>200&&Math.abs(split.preview-split.editor)<2,JSON.stringify(split));assert.equal(split.overflow,false);
  // Source offset of a semantic heading, independent of unequal total heights.
  const point=await page.evaluate(()=>{
    const node=[...document.querySelectorAll('#preview h2')].find(el=>el.textContent==='段落 44');
    const editor=document.querySelector('#editor'),source=editor.value,offset=Number(node.dataset.sourceStart);
    editor.focus();editor.setSelectionRange(offset,offset);editor.dispatchEvent(new KeyboardEvent('keyup',{key:'ArrowDown',bubbles:true}));
    return {offset,previewTop:node.getBoundingClientRect().top};
  });
  await page.waitForTimeout(120);
  assert.ok(await page.evaluate(()=>{
    const node=[...document.querySelectorAll('#preview h2')].find(el=>el.textContent==='段落 44');
    const pane=document.querySelector('#preview').getBoundingClientRect(),rect=node.getBoundingClientRect();return rect.top>=pane.top&&rect.top<pane.bottom;
  }),'caret reveals matching preview heading');
  await page.evaluate(()=>{const pane=document.querySelector('#preview'),node=[...pane.querySelectorAll('h2')].find(el=>el.textContent==='段落 25');pane.scrollTop+=node.getBoundingClientRect().top-pane.getBoundingClientRect().top;});
  await page.waitForTimeout(150);
  const alignment=await page.evaluate(()=>{
    const pane=document.querySelector('#preview'),node=[...pane.querySelectorAll('h2')].find(el=>el.textContent==='段落 25');
    const mirror=document.querySelector('div[style*="-100000px"]'),range=document.createRange(),offset=Number(node.dataset.sourceStart);
    range.setStart(mirror.firstChild,offset);range.setEnd(mirror.firstChild,offset+1);
    return {offset,editorTop:document.querySelector('#editor').scrollTop,expected:range.getBoundingClientRect().top-mirror.getBoundingClientRect().top};
  });
  assert.ok(Math.abs(alignment.editorTop-alignment.expected)<3,JSON.stringify(alignment));
  await page.evaluate(()=>{const editor=document.querySelector('#editor');editor.scrollTop+=180;});await page.waitForTimeout(100);
  const stable=await page.evaluate(()=>[document.querySelector('#preview').scrollTop,document.querySelector('#editor').scrollTop]);
  await page.waitForTimeout(180);
  assert.deepEqual(await page.evaluate(()=>[document.querySelector('#preview').scrollTop,document.querySelector('#editor').scrollTop]),stable,'no scroll feedback loop');
  await page.keyboard.press('Control+h');await page.locator('#find-input').fill('段落 70');
  await page.waitForFunction(()=>document.querySelector('#find-state').textContent==='1/1');
  assert.equal(await page.locator('#editor').evaluate(editor=>editor.value.slice(editor.selectionStart,editor.selectionEnd)),'段落 70');
  await page.waitForFunction(()=>{
    const node=[...document.querySelectorAll('#preview h2')].find(el=>el.textContent==='段落 70');
    const pane=document.querySelector('#preview').getBoundingClientRect(),rect=node.getBoundingClientRect();return rect.top>=pane.top-5&&rect.top<pane.bottom;
  });
  await page.locator('#find-close').evaluate(button=>button.click());
  await page.screenshot({path:resolve(temp,'split-desktop.png')});
  await page.setViewportSize({width:390,height:844});await page.waitForTimeout(120);
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
  await page.screenshot({path:resolve(temp,'split-narrow.png')});
  await page.emulateMedia({media:'print'});
  assert.equal(await page.locator('#app').evaluate(element=>getComputedStyle(element).position),'static');
  assert.ok(await page.locator('#preview').evaluate(element=>element.clientHeight>innerHeight*2),'split print uses full document flow');
  await page.emulateMedia({media:'screen'});
  await page.setViewportSize({width:1100,height:800});await page.locator('#btn-split').evaluate(button=>button.click());
  await page.locator('#preview .mdp-mermaid').last().click();await page.locator('[data-diagram-edit]').click();
  await page.waitForFunction(()=>!document.querySelector('.mdp-diagram-editor select').disabled);
  await page.locator('.mdp-lightbox g.node').filter({hasText:'检查'}).click();
  assert.equal(await page.locator('.mdp-diagram-editor select').inputValue(),'B','select node by clicking the diagram');
  assert.equal(await page.locator('.mdp-lightbox g.node.mdp-node-selected').count(),1);
  await page.locator('.mdp-diagram-editor textarea').fill('图上修改 "节点" [中文] 🙂\n下一行');
  await page.locator('[data-diagram-save]').click();
  await page.waitForFunction(()=>document.querySelector('[data-diagram-status]').textContent.includes('已应用'));
  await page.keyboard.press('Control+s');await savedContains('图上修改');
  let contents=await readFile(path,'utf8');
  assert.equal(contents.includes(graph),true,'first identical diagram unchanged');
  assert.equal(contents.match(/图上修改/g).length,1,'only the selected second diagram changed');
  await page.locator('.mdp-diagram-editor select').selectOption('B');await page.locator('[data-diagram-delete]').click();await page.locator('[data-diagram-confirm]').click();
  await page.waitForFunction(()=>!Array.from(document.querySelector('.mdp-diagram-editor select').options).some(option=>option.value==='B'));
  await page.keyboard.press('Control+s');await page.waitForTimeout(250);
  await page.locator('[data-diagram-undo]').click();
  await page.waitForFunction(()=>Array.from(document.querySelector('.mdp-diagram-editor select').options).some(option=>option.value==='B'));
  await page.screenshot({path:resolve(temp,'diagram-desktop.png')});
  await page.setViewportSize({width:390,height:844});await page.screenshot({path:resolve(temp,'diagram-narrow.png')});
  assert.ok(await page.locator('[data-lb="close"]').evaluate(button=>{const rect=button.getBoundingClientRect();return rect.right<=innerWidth&&rect.top>=0;}),'narrow layout keeps close button accessible');
  await page.keyboard.press('Escape');await page.setViewportSize({width:1100,height:800});
  // Same graph operation must remain available while rich text editing.
  await page.locator('#btn-preview-edit').evaluate(button=>button.click());await page.waitForSelector('body.preview-editing');
  await page.waitForSelector('#rich-editor .mdp-mermaid svg');
  await page.locator('#rich-editor .mdp-mermaid').last().click();await page.locator('[data-diagram-edit]').click();
  await page.waitForFunction(()=>!document.querySelector('.mdp-diagram-editor select').disabled);
  await page.locator('.mdp-diagram-editor select').selectOption('A');await page.locator('.mdp-diagram-editor textarea').fill('富文本中修改图节点');await page.locator('[data-diagram-save]').click();
  await page.waitForFunction(()=>document.querySelector('[data-diagram-status]').textContent.includes('已应用'));
  await page.keyboard.press('Control+s');await savedContains('富文本中修改图节点');
  assert.equal(await page.locator('body').evaluate(body=>body.classList.contains('preview-editing')),true);
  assert.deepEqual(errors,[]);
  console.log('[preview-workflows] OK: semantic split, diagram labels/delete/undo, duplicate diagrams, rich editing; evidence '+temp);
} finally {if(browser)await browser.close();if(child.exitCode===null)child.kill();}
