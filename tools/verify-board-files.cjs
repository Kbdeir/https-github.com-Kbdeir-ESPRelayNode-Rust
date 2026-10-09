const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const assert = require('node:assert/strict');
const base = process.env.SMARTCONFIG_BOARD_URL;
assert.ok(base, 'Set SMARTCONFIG_BOARD_URL to the board URL');
const credentials = JSON.parse(fs.readFileSync(path.join(__dirname, '..', 'config.local.json'), 'utf8')).management;
const headers = {Authorization: 'Basic ' + Buffer.from(credentials.username + ':' + credentials.password).toString('base64')};
const scratch = '/verify-' + crypto.randomBytes(8).toString('hex') + '.txt';
const renamed = scratch.replace('.txt', '-renamed.txt');
const created = new Set();
async function request(uri, options = {}) {
  return fetch(base + uri, {...options, headers: {...headers, ...options.headers}, signal: AbortSignal.timeout(20000)});
}
const contentUri = name => '/api/files/content?path=' + encodeURIComponent(name);
async function remove(name) {
  const response = await request('/api/files/delete', {method: 'POST', headers: {'X-SmartConfig': '1'}, body: new URLSearchParams({path: name})});
  await response.text();
  return response;
}
(async () => {
  for (const name of [scratch, renamed]) {
    const response = await request(contentUri(name));
    assert.equal(response.status, 404, 'Scratch name must not already exist');
    await response.text();
  }
  const before = await (await request('/api/files/list')).json();
  assert.equal(before.fsTotal, 786432);
  const payload = Buffer.from('streamed LittleFS hardware check\n'.repeat(3200));
  try {
    for (const data of [payload, Buffer.from('replacement saved atomically\n')]) {
      created.add(scratch);
      const upload = await request(contentUri(scratch), {method: 'POST', headers: {'X-SmartConfig': '1', 'Content-Type': 'application/octet-stream'}, body: data});
      const message = await upload.text();
      assert.equal(upload.status, 200, message);
      const download = await request(contentUri(scratch));
      assert.equal(download.status, 200);
      assert.deepEqual(Buffer.from(await download.arrayBuffer()), data);
    }
    const rename = await request('/api/files/rename', {method: 'POST', headers: {'X-SmartConfig': '1'}, body: new URLSearchParams({from: scratch, to: renamed})});
    const message = await rename.text();
    assert.equal(rename.status, 200, message);
    created.delete(scratch);
    created.add(renamed);
    const download = await request(contentUri(renamed));
    assert.equal(download.status, 200);
    assert.equal(await download.text(), 'replacement saved atomically\n');
    assert.equal((await remove(renamed)).status, 200);
    created.delete(renamed);
    const missing = await request(contentUri(renamed));
    assert.equal(missing.status, 404);
    await missing.text();
    const after = await (await request('/api/files/list')).json();
    assert.deepEqual(after.files, before.files, 'All original assets must be unchanged');
    console.log(JSON.stringify({result: 'LittleFS hardware file checks passed', uploadBytes: payload.length, assets: after.files.length, fsTotal: after.fsTotal, fsUsed: after.fsUsed}));
  } finally {
    for (const name of created) {
      const response = await remove(name);
      assert.ok([200, 404].includes(response.status), 'Scratch cleanup failed: ' + name);
    }
  }
})().catch(error => {console.error(error.message); process.exitCode = 1;});
