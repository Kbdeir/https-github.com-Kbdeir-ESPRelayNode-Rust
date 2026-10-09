(function () {
  'use strict';
  var responsive=document.createElement('style');
  responsive.textContent='@media(max-width:920px){.mqtt-wide,.wireguard-wide,.location-wide{grid-column:auto}.grid>div,.card{min-width:0}.wifi-scan-row{grid-template-columns:minmax(0,1fr) auto}}';
  document.head.appendChild(responsive);
  function request(url, body, json) {
    return fetch(url, {method: 'POST', headers: {'X-SmartConfig': '1', 'Content-Type': json ? 'application/json' : 'application/x-www-form-urlencoded'}, body: json ? JSON.stringify(body) : body.toString()})
      .then(function (r) { return r.json().then(function (d) { if (!r.ok || !d.ok) throw new Error(d.message || 'Request failed'); return d; }); });
  }
  // Original form layout is unchanged; mutations use POST instead of credential-bearing GET URLs.
  document.addEventListener('submit', function (event) {
    var form = event.target, path = new URL(form.action, location.href).pathname;
    if (['/Apply.html', '/ApplyRelay.html', '/ApplyIRMap.html', '/savetimer.html', '/RelayConfig.html'].indexOf(path) < 0) return;
    event.preventDefault();
    var fields = new URLSearchParams(new FormData(form));
    if (event.submitter && event.submitter.name) fields.set(event.submitter.name, event.submitter.value);
    request(path, fields, false).then(function (d) {
      if (d.restart) { alert(d.message); setTimeout(function () { location.href = path === '/savetimer.html' ? '/Timer1?GetTimer=' + fields.get('TNumber') : '/'; }, 7000); }
      else {
        var status = document.getElementById('save-status');
        if (status) status.textContent = d.message;
        if (typeof fetchAndApplyConfig === 'function') fetchAndApplyConfig();
      }
    }).catch(function (e) { alert(e.message); });
  }, true);
  var originalSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.send = function (body) { if (body && this._rustPost) this.setRequestHeader('X-SmartConfig', '1'); return originalSend.call(this, body); };
  var originalOpen = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method) { this._rustPost = String(method).toUpperCase() === 'POST'; return originalOpen.apply(this, arguments); };
  document.addEventListener('DOMContentLoaded', function () {
    var reboot = document.getElementById('reboot');
    if (reboot) reboot.addEventListener('click', function () {
      if (!confirm('Reboot the controller? The relay will be switched OFF.')) return;
      reboot.disabled = true;
      request('/api/reboot', new URLSearchParams(), false).then(function (d) {
        document.getElementById('save-status').textContent = d.message;
        setTimeout(function () { location.reload(); }, 7000);
      }).catch(function (e) { reboot.disabled = false; alert(e.message); });
    });
    ['CEnabled', 'CSunday', 'CMonday', 'CTuesday', 'CWednesday', 'CThursday', 'CFriday', 'CSaturday'].forEach(function (id) { var e = document.getElementById(id); if (e) e.checked = e.value === '1'; });
    ['pass', 'mqttPass'].forEach(function (id) { var e = document.getElementById(id); if (e) { e.type = 'password'; e.placeholder = 'Unchanged if blank'; } });
    var active = ['ssid', 'pass', 'PhyLoc', 'timeserver', 'ntptz', 'MQTT_Active', 'MQTT_BROKER', 'MQTT_B_PRT', 'MQTT_KeepAliveSeconds', 'mqttUser', 'mqttPass', 'webPassword'];
    var systemForm = document.querySelector('form[action="Apply.html"]');
    if(systemForm&&!document.getElementById('webPassword')){var grid=systemForm.querySelector('.form-grid');var field=document.createElement('div');field.className='field';var label=document.createElement('label');label.htmlFor='webPassword';label.textContent='Web Password';var password=document.createElement('input');password.id='webPassword';password.name='webPassword';password.type='password';password.minLength=8;password.maxLength=63;password.placeholder='Unchanged if blank';field.append(label,password);grid.appendChild(field);}
    if (systemForm) systemForm.querySelectorAll('input,select').forEach(function (e) { if (e.name && e.type !== 'submit' && active.indexOf(e.name) < 0) { e.disabled = true; e.title = 'Not ported yet'; } });
    ['ACS_AMPS','LWILL_TOPIC','SUB_TOPIC1','TemperatureValue_edt','AlexaName','ACS_elasticity','Max_Current','ACSmultiple','ACS_Active'].forEach(function (id) { var e = document.getElementById(id); if(e) { e.disabled = true; e.title = 'Not ported yet'; } });
    document.querySelectorAll('a[href]').forEach(function (a) {
      var p = new URL(a.href, location.href).pathname;
      if (/VPN|CurrentConfig|coredump|Reset|SerialLog|AdvancedTemperature/.test(p)) { a.classList.add('disabled'); a.setAttribute('aria-disabled', 'true'); a.addEventListener('click', function (e) { e.preventDefault(); }); a.title = 'Not ported yet'; }
    });
    if (typeof controlRelay === 'function' && typeof boards !== 'undefined') {
      window.controlRelay = function (boardId, relay, action) { var b = boards[boardId]; request(b.base + '/api/relay', {relay: relay, action: action}, true).catch(function(e) { alert(e.message); }); };
    }
    if (typeof tmrControl === 'function' && typeof boards !== 'undefined') {
      window.tmrControl = function(boardId, relay, action) { var b=boards[boardId]; request(b.base+'/TimerControl.json',new URLSearchParams({relay:relay,action:action}),false).then(function(){pollBoard(b);}).catch(function(e){alert(e.message);}); };
    }
    document.querySelectorAll('button,input[type="submit"]').forEach(function(e){if(e.id==='ResetAccumulatedPowerButton'){e.disabled=true;e.title='Not ported yet';}});
    document.querySelectorAll('button[onclick],input[onclick]').forEach(function(e){if(/VPN|Firmware|Backup|Reset|CurrentConfig/.test(e.getAttribute('onclick'))){e.disabled=true;e.title='Not ported yet';}else if(/Automation|RemoteSensors/.test(e.getAttribute('onclick'))){e.onclick=function(){location.href='/Automation.html';};}else if(/Files/.test(e.getAttribute('onclick'))){e.onclick=function(){location.href='/Files.html';};}});
  });
}());
