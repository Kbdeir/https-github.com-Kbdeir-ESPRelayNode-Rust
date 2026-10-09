const {chromium} = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '..');
const board = Boolean(process.env.SMARTCONFIG_BOARD_URL);
const base = process.env.SMARTCONFIG_BOARD_URL || process.env.SMARTCONFIG_PREVIEW_URL || 'http://127.0.0.1:8090';
if (!board) assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(new URL(base).hostname), 'Mutating preview checks are loopback-only; use SMARTCONFIG_BOARD_URL for read-only hardware checks');
const credentials = board ? JSON.parse(fs.readFileSync(path.join(root, 'config.local.json'), 'utf8')).management : {username: 'user', password: 'preview-local'};
const output = path.join(root, 'target', board ? 'board-ui-check' : 'ui-check');
fs.mkdirSync(output, {recursive: true});
(async () => {
  const browser = await chromium.launch({channel: 'msedge', headless: true});
  const context = await browser.newContext({httpCredentials: {username: credentials.username, password: credentials.password}});
  const unauthorized = await browser.newContext();
  assert.equal((await unauthorized.request.get(base + '/')).status(), 401, 'Unauthenticated page must be rejected');
  await unauthorized.close();
  assert.equal((await context.request.get(base + '/rust-ui.js')).status(), 200, 'UI adapter must load');
  if (board) {
    const response = await context.request.get(base + '/api/status', {headers: {'X-Header-Size-Test': 'x'.repeat(900)}});
    assert.equal(response.status(), 200, 'Authenticated headers exceeding 512 bytes must work');
  }
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', e => errors.push(e.message));
  page.on('dialog', d => d.type() === 'prompt' ? d.accept('/browser-renamed.txt') : d.accept());
  const pages = ['/', '/RelayConfig.html', '/InputsConfig.html', '/Input_Relays_Map.html', '/Timer1?GetTimer=1', '/LiveReadings.html', '/LiveReadingsRest.html', '/ModbusConfig.html', '/Automation.html', '/Files.html', '/FirmwareMaintenance.html', '/Backup.html'];
  for (const viewport of [{width: 1440, height: 1000}, {width: 390, height: 844}]) {
    await page.setViewportSize(viewport);
    for (const uri of pages) {
      const response = await page.goto(base + uri);
      assert.equal(response.status(), 200, `${uri} must be served successfully`);
      await page.waitForTimeout(400);
      const dimensions = await page.evaluate(() => ({width: innerWidth, scroll: document.documentElement.scrollWidth, title: document.title, unresolved: /%[A-Za-z_][A-Za-z_0-9]*%/.test(document.body.innerText)}));
      assert.ok(dimensions.scroll <= dimensions.width + 1, `${uri} overflows at ${viewport.width}: ${dimensions.scroll}`);
      assert.equal(dimensions.unresolved, false, `${uri} has unresolved template text`);
      if (uri === '/Backup.html') {
        await page.waitForFunction(() => /^[a-f0-9]{6,12}$/i.test(document.getElementById('this-chipid').textContent));
      }
      if (uri === '/LiveReadings.html') {
        await page.waitForFunction(() => ['t-heap','t-heap-min','t-heap-blk'].every(id => /^\d[\d,\s.]*$/.test(document.getElementById(id).textContent)));
      }
      if (uri === '/') {
        assert.equal(await page.locator('#reboot').inputValue(), 'Reboot');
        assert.ok(await page.locator('#reboot').isEnabled());
        const actions = await page.locator('#reboot').evaluate(button => {
          const save=button.previousElementSibling.getBoundingClientRect(), reboot=button.getBoundingClientRect();
          return {nextToSave:button.previousElementSibling.value==='Save',overlap:save.right>reboot.left&&save.top<reboot.bottom&&save.bottom>reboot.top};
        });
        assert.ok(actions.nextToSave);
        assert.equal(actions.overlap,false);
        const layout = await page.locator('#modbus-link').evaluate(link => {
          const bounds=link.getBoundingClientRect(), previous=link.previousElementSibling.getBoundingClientRect(), next=link.nextElementSibling.getBoundingClientRect();
          return {inTools:!!link.closest('.link-list'), duplicates:document.querySelectorAll('#modbus-link').length, overlaps:bounds.top<previous.bottom || bounds.bottom>next.top};
        });
        assert.equal(layout.inTools,true,'Modbus link belongs in Tools');
        assert.equal(layout.duplicates,1);
        assert.equal(layout.overlaps,false,'Modbus link overlaps adjacent controls');
        for(const href of ['/Backup.html','/FirmwareMaintenance.html']) {
          const link=page.locator(`a[href=".${href}"]`);
          assert.notEqual(await link.getAttribute('aria-disabled'),'true');
        }
      }
      const filename = `${viewport.width}-${uri.replace(/[^a-z0-9]/gi, '_') || 'config'}.png`;
      await page.screenshot({path: path.join(output, filename), fullPage: true});
      if (uri === '/Automation.html') {
        await page.getByRole('button', {name: 'Remote Sensors', exact: true}).click();
        assert.equal(await page.locator('form.remote').count(), 8);
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
        await page.screenshot({path: path.join(output, `${viewport.width}-remote-sensors.png`), fullPage: true});
      }
    }
  }
  if (!board) {
  const factoryWifi=(await (await context.request.get(base+'/GetConfig.json')).json()).ssid;
  assert.equal((await context.request.get(base+'/api/reboot')).status(),405);
  assert.equal((await context.request.post(base+'/api/reboot',{data:''})).status(),403);
  await page.goto(base+'/');
  await page.waitForFunction(()=>document.getElementById('PhyLoc').value==='Rust Local Preview');
  await page.locator('#PhyLoc').fill('Live save browser test');
  const saved=page.waitForResponse(r=>new URL(r.url()).pathname==='/Apply.html'&&r.request().method()==='POST');
  await page.getByRole('button',{name:'Save',exact:true}).click();
  assert.equal((await (await saved).json()).restart,false);
  await page.waitForFunction(()=>document.getElementById('save-status').textContent.includes('Saved'));
  assert.equal((await (await context.request.get(base+'/api/status')).json()).restart_pending,false);
  assert.equal((await (await context.request.get(base+'/GetConfig.json')).json()).PhyLoc,'Live save browser test');
  await page.screenshot({path:path.join(output,'mobile-live-save.png'),fullPage:true});
  await page.goto(base+'/RelayConfig.html');
  await page.getByRole('button',{name:'ON',exact:true}).click();
  await page.waitForTimeout(200);
  const liveRelaySave=await context.request.post(base+'/ApplyRelay.html',{headers:{'X-SmartConfig':'1'},form:{RELAYNB:'0',ttl:'60',tta:'4'}});
  assert.equal((await liveRelaySave.json()).restart,false);
  await page.waitForTimeout(3200);
  const live=await (await context.request.get(base+'/api/status')).json();
  assert.equal(live.restart_pending,false);
  assert.equal(live.runtime.relay.on,true,'Non-Wi-Fi save must not reboot or reset the relay');
  assert.equal(live.runtime.ttl_seconds,60,'Saved TTL must reach the running engine');
  assert.equal(live.runtime.tta_seconds,4,'Saved TTA must reach the running engine');
  const shorter=await context.request.post(base+'/ApplyRelay.html',{headers:{'X-SmartConfig':'1'},form:{RELAYNB:'0',ttl:'1',tta:'4'}});
  assert.equal((await shorter.json()).restart,false);
  await page.waitForTimeout(150);
  assert.equal((await (await context.request.get(base+'/api/relay/state')).json()).on,false,'Shortening TTL applies to original ON time immediately');
  await context.request.post(base+'/ApplyRelay.html',{headers:{'X-SmartConfig':'1'},form:{RELAYNB:'0',ttl:'0',tta:'0'}});
  await page.goto(base+'/');
  const rebootResponse=page.waitForResponse(r=>new URL(r.url()).pathname==='/api/reboot'&&r.request().method()==='POST');
  await page.locator('#reboot').click();
  assert.equal((await (await rebootResponse).json()).restart,true);
  assert.equal((await (await context.request.get(base+'/api/status')).json()).restart_pending,true);
  await page.waitForTimeout(3500);
  const backupResponse=await context.request.get(base+'/api/backup?includeWeb=1');
  assert.equal(backupResponse.status(),200);
  const backupBytes=await backupResponse.body();
  const backup=JSON.parse(backupBytes);
  assert.ok(backup['/rust-config.json']);
  assert.ok(backup['/FirmwareMaintenance.html']);
  await page.goto(base+'/Backup.html');
  const backupDownload=page.waitForEvent('download');
  await page.locator('#btn-download').click();
  assert.equal((await backupDownload).suggestedFilename(),'smartconfig-backup.json');
  await page.locator('#file-input').setInputFiles({name:'restore.json',mimeType:'application/json',buffer:backupBytes});
  await page.waitForFunction(()=>!document.querySelector('#btn-restore').disabled);
  await page.locator('#btn-restore').click();
  await page.waitForFunction(()=>document.querySelector('#restore-status').classList.contains('ok'));
  await page.waitForTimeout(3500);
  await page.goto(base+'/Backup.html');
  await page.locator('#file-input').setInputFiles({name:'bad.json',mimeType:'application/json',buffer:Buffer.from('not JSON')});
  await page.waitForFunction(()=>document.querySelector('#restore-status').classList.contains('bad'));
  assert.ok(await page.locator('#btn-restore').isDisabled());
  await page.goto(base+'/FirmwareMaintenance.html');
  await page.locator('#FileUpload1').setInputFiles({name:'bad.bin',mimeType:'application/octet-stream',buffer:Buffer.from('not firmware')});
  const uploadRequest=page.waitForRequest(request=>request.method()==='POST'&&new URL(request.url()).pathname==='/update');
  await page.locator('input[name="firmwareload"]').click();
  const upload=await uploadRequest;
  assert.equal(upload.headers()['x-smartconfig'],'1');
  assert.equal(upload.headers()['content-type'],'application/octet-stream');
  assert.equal(upload.postData(),'not firmware');
  await page.waitForFunction(()=>document.querySelector('#FirmwareUploadText').textContent.includes('failed'));
  assert.equal(await page.locator('form[action="/update"]').getAttribute('data-uploading'),null);
  await page.goto(base + '/Files.html');
  const fileContent = 'first line\n' + 'large streamed upload\n'.repeat(1000);
  await page.locator('#upload-file').setInputFiles({name: 'browser-file.txt', mimeType: 'text/plain', buffer: Buffer.from(fileContent)});
  await page.locator('#btn-upload').click();
  await page.waitForFunction(() => document.querySelector('#upload-status').classList.contains('ok'));
  await page.getByRole('button', {name: '/browser-file.txt', exact: true}).click();
  await page.waitForFunction(() => document.querySelector('#editor-content').value.startsWith('first line'));
  await page.locator('#editor-content').fill('edited in browser');
  await page.locator('#btn-save').click();
  await page.waitForFunction(() => document.querySelector('#editor-status').classList.contains('ok'));
  assert.equal(await (await context.request.get(base + '/api/files/content?path=%2Fbrowser-file.txt')).text(), 'edited in browser');
  await page.locator('#file-filter').fill('browser-file');
  await page.locator('#chk-select-all').check();
  await page.locator('#btn-rename-sel').click();
  await page.waitForFunction(() => !document.querySelector('#file-rows').textContent.includes('browser-file'));
  assert.equal(await (await context.request.get(base + '/api/files/content?path=%2Fbrowser-renamed.txt')).text(), 'edited in browser');
  await page.locator('#file-filter').fill('browser-renamed');
  await page.locator('#chk-select-all').check();
  const downloadEvent = page.waitForEvent('download');
  await page.locator('#btn-download-sel').click();
  assert.equal((await downloadEvent).suggestedFilename(), 'browser-renamed.txt');
  await page.locator('#btn-delete-sel').click();
  await page.waitForFunction(() => !document.querySelector('#file-rows').textContent.includes('browser-renamed'));
  assert.equal((await context.request.get(base + '/api/files/content?path=%2Fbrowser-renamed.txt')).status(), 404);
  await page.locator('#upload-file').setInputFiles({name:'browser-static.html', mimeType:'text/html', buffer:Buffer.from('<html><body>Compressed browser asset</body></html>')});
  await page.locator('#upload-gzip').check();
  await page.locator('#btn-upload').click();
  await page.waitForFunction(() => document.querySelector('#upload-status').classList.contains('ok'));
  const compressed = await context.request.get(base + '/browser-static.html');
  assert.equal(compressed.headers()['content-encoding'], 'gzip');
  assert.equal(await compressed.text(), '<html><body>Compressed browser asset</body></html>');
  assert.equal((await context.request.post(base + '/api/files/delete', {headers:{'X-SmartConfig':'1'}, form:{path:'/browser-static.html.gz'}})).status(),200);
  await page.goto(base + '/RelayConfig.html');
  await page.getByRole('button', {name: 'ON', exact: true}).click();
  await page.waitForTimeout(250);
  assert.equal((await (await context.request.get(base + '/api/relay/state')).json()).state, 'ON');
  await page.getByRole('button', {name: 'OFF', exact: true}).click();
  await page.waitForTimeout(250);
  assert.equal((await (await context.request.get(base + '/api/relay/state')).json()).state, 'OFF');
  await page.goto(base + '/Timer1?GetTimer=1');
  await page.locator('#CEnabled').check();
  await page.locator('#FTime').fill('08:00');
  await page.locator('#TTime').fill('09:00');
  await page.getByRole('button', {name: 'Save Timer', exact: true}).click();
  await page.waitForTimeout(100);
  assert.equal((await (await context.request.get(base + '/TimerStatus.json')).json()).timers[0].enabled, true);
  assert.equal((await (await context.request.get(base+'/api/status')).json()).restart_pending,false);
  await page.goto(base + '/Automation.html');
  await page.locator('#c0source').selectOption('7');
  await page.locator('#c0v1').fill('on');
  await page.getByRole('button', {name: 'Save Rule', exact: true}).click();
  await page.waitForTimeout(150);
  assert.equal((await (await context.request.get(base + '/api/automation')).json()).rules[0].conditions[0].source, 7);
  assert.equal((await (await context.request.get(base+'/api/status')).json()).restart_pending,false);
  await page.goto(base + '/Automation.html');
  await page.getByRole('button', {name: 'Remote Sensors', exact: true}).click();
  const remote = page.locator('form.remote').first();
  await remote.locator('input[name=topic]').fill('test/remote/value');
  await remote.getByRole('button', {name: 'Save Sensor', exact: true}).click();
  await page.waitForTimeout(150);
  assert.equal((await (await context.request.get(base + '/api/automation')).json()).remotes[0].topic, 'test/remote/value');
  assert.equal((await (await context.request.get(base+'/api/status')).json()).restart_pending,false);
  await page.goto(base + '/Backup.html');
  const resetRequest = page.waitForRequest(request => request.method() === 'POST' && new URL(request.url()).pathname === '/api/resetconfig');
  await page.locator('#btn-reset-config').click();
  assert.equal((await resetRequest).headers()['x-smartconfig'], '1');
  await page.waitForFunction(() => document.querySelector('#reset-status').classList.contains('ok'));
  await page.waitForTimeout(3500);
  const defaults = await (await context.request.get(base + '/GetConfig.json')).json();
  for (const [key, suffix] of Object.entries({PUB_TOPIC1:'/Coils/C0',STATE_PUB_TOPIC:'/Coils/State/C0',TTL_PUB_TOPIC:'/sts/VTTL0',i_ttl_PUB_TOPIC:'/i/TTL0',CURR_TTL_PUB_TOPIC:'/sts/CURRVTTL0'})) {
    assert.equal(defaults[key], `/home/Controller${defaults.NODEID}${suffix}`);
  }
  for (const [index,key] of ['I12_STS_PTP','I14_STS_PTP','I03_STS_PTP','I04_STS_PTP','I05_STS_PTP','I06_STS_PTP'].entries()) {
    assert.equal(defaults[key], `/home/Controller${defaults.NODEID}/INS/sts/IN${index+1}`);
    assert.equal(defaults[`I${index+1}MODE`],2);
  }
  assert.equal(defaults.PhyLoc,'Not configured yet');
  assert.equal(defaults.ssid,factoryWifi);
  assert.equal(defaults.MQTT_BROKER,'192.168.1.1');
  assert.equal(defaults.MQTT_Active,0);
  assert.equal(defaults.timeserver,'162.159.200.123');
  assert.equal(defaults.ntptz,2);
  const resetTimers = await (await context.request.get(base + '/TimerStatus.json')).json();
  assert.ok(resetTimers.timers.every(timer=>!timer.enabled));
  assert.equal(resetTimers.timers[0].timeFrom,'08:00');
  const resetRules = await (await context.request.get(base + '/api/automation')).json();
  assert.ok(resetRules.rules.every(rule=>!rule.enabled));
  assert.equal(resetRules.rules[0].conditions[0].source,7);
  await page.goto(base + '/RelayConfig.html');
  await page.screenshot({path:path.join(output,'reset-default-relay.png'),fullPage:true});
  }
  assert.deepEqual(errors, []);
  for (const filename of ['config.html','RelayConfig.html','InputsConfig.html','Input_Relays_Map.html','Timer1.html','LiveReadings.html','LiveReadingsRest.html','Automation.html','Files.html','FirmwareMaintenance.html','Backup.html']) {
    const referencePath = ['Automation.html', 'Files.html'].includes(filename) ? 'C:/Users/kbdeir/Documents/PlatformIO/Projects/ESP-IDF-ESPRelayNode8266/data' : 'C:/Users/kbdeir/Documents/PlatformIO/Projects/SmartConfig - AI/data';
    const reference = fs.readFileSync(path.join(referencePath, filename), 'utf8');
    const ported = fs.readFileSync(path.join(root, 'web', filename), 'utf8');
    const styles = text => Array.from(text.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/gi), m => m[1]);
    assert.deepEqual(styles(ported), styles(reference), `${filename} changed reference CSS`);
  }
  await browser.close();
  console.log(`UI checks passed: ${pages.length} pages at two widths; authentication; ${board ? 'read-only board checks' : 'simulated relay controls, timer/rule/sensor saves, reset default generation'}; reference CSS unchanged. Screenshots: ${output}`);
})().catch(e => {console.error(e); process.exit(1);});
