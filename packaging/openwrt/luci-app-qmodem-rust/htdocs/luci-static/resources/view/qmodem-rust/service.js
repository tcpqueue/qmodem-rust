'use strict';
'require view';
'require rpc';
'require fs';
'require ui';
'require poll';

var serviceName = 'qmodem-rust';
var list = rpc.declare({ object: 'rc', method: 'list', params: ['name'], expect: { '': {} } });
var control = rpc.declare({ object: 'rc', method: 'init', params: ['name', 'action'] });
var procd = rpc.declare({ object: 'service', method: 'list', params: ['name'], expect: { '': {} } });
function statusData() { return Promise.all([list(serviceName), procd(serviceName)]); }
function execute(args) {
    return fs.exec('/usr/sbin/qmodemd', ['--config', '/etc/qmodem-rust.toml'].concat(args)).then(function(result) {
        if (result.code !== 0) throw new Error(result.stderr || _('Command failed'));
        return JSON.parse(result.stdout);
    });
}
function notify(error) { ui.addNotification(null, E('p', {}, error.message || String(error)), 'error'); }
function field(label, input, description) {
    return E('div', { 'class': 'cbi-value' }, [
        E('label', { 'class': 'cbi-value-title', 'for': input.id }, label),
        E('div', { 'class': 'cbi-value-field' }, [input, E('div', { 'class': 'cbi-value-description' }, description || '')])
    ]);
}
function select(id, choices, selected) {
    return E('select', { 'id': id, 'class': 'cbi-input-select', 'disabled': !L.hasViewPermission() }, choices.map(function(item) {
        return E('option', { 'value': item[0], 'selected': item[0] === selected }, item[1]);
    }));
}
return view.extend({
    load: function() {
        return Promise.all([execute(['service-info']), statusData(), execute(['interfaces'])]);
    },
    render: function(data) {
        var cfg = data[0], status = E('span'), authConfigured = cfg.auth_configured;
        var readonly = !L.hasViewPermission();
        var dashboardHost = cfg.listen;
        if (['0.0.0.0', '::', '127.0.0.1', '::1'].indexOf(dashboardHost) !== -1)
            dashboardHost = window.location.hostname;
        if (dashboardHost.indexOf(':') !== -1 && dashboardHost.charAt(0) !== '[')
            dashboardHost = '[' + dashboardHost + ']';
        var dashboardUrl = 'http://' + dashboardHost + ':' + cfg.port + '/';
        var listen = E('input', { 'id': 'qmr-listen', 'class': 'cbi-input-text', 'type': 'text', 'value': cfg.listen, 'disabled': readonly });
        var port = E('input', { 'id': 'qmr-port', 'class': 'cbi-input-text', 'type': 'number', 'min': '1', 'max': '65535', 'value': cfg.port, 'disabled': readonly });
        var choices = [['any', _('All network devices')]];
        (data[2].interfaces || []).forEach(function(device) {
            choices.push([device.name, device.name + (device.addresses.length ? ' (' + device.addresses.join(', ') + ')' : '')]);
        });
        if (cfg.interface && !choices.some(function(c) { return c[0] === cfg.interface; }))
            choices.push([cfg.interface, cfg.interface + ' — ' + _('Currently unavailable')]);
        var device = select('qmr-interface', choices, cfg.interface || 'any');
        var level = select('qmr-log-level', [
            ['error', _('Error')], ['warn', _('Warning')], ['info', _('Information')],
            ['debug', _('Debug')], ['trace', _('Trace')], ['off', _('Off')]
        ], cfg.log_level);
        var format = select('qmr-log-format', [['text', _('Text')], ['json', 'JSON']], cfg.log_format);
        var update = function(services) {
            var entry = services[0][serviceName] || {};
            var instances = (services[1][serviceName] || {}).instances || {};
            var running = Object.keys(instances).some(function(key) { return instances[key].running; });
            status.textContent = (running ? _('Running') : _('Stopped')) + ' / ' + (entry.enabled ? _('Autostart enabled') : _('Autostart disabled'));
        };
        update(data[1]);
        var pollFailed = false;
        poll.add(function() { return statusData().then(function(s) { pollFailed = false; update(s); }).catch(function(error) {
            status.textContent = _('Status unavailable');
            if (!pollFailed) notify(error);
            pollFailed = true;
        }); }, 5);
        var button = function(label, action) {
            return E('button', { 'class': 'cbi-button cbi-button-action', 'disabled': readonly, 'click': ui.createHandlerFn(this, function() {
                return control(serviceName, action).then(function(result) {
                    if (result) throw new Error(_('Service action failed'));
                    return statusData().then(update);
                }).catch(notify);
            }) }, label);
        }.bind(this);
        var tokenState = E('span', {}, authConfigured ? _('Configured') : _('Not configured'));
        var createToken = E('button', { 'class': 'cbi-button cbi-button-action', 'disabled': readonly || authConfigured, 'click': ui.createHandlerFn(this, function() {
            return execute(['init-auth']).then(function(result) {
                authConfigured = true;
                tokenState.textContent = _('Configured');
                createToken.disabled = true;
                ui.showModal(_('Access token'), [
                    E('p', {}, _('Save this token now. It is shown only once. The server stores its hash. Restart the service after saving settings.')),
                    E('textarea', { 'class': 'cbi-input-textarea', 'readonly': true, 'rows': 2, 'aria-label': _('Access token') }, result.token),
                    E('div', { 'class': 'right' }, E('button', { 'class': 'cbi-button', 'click': ui.hideModal }, _('Close')))
                ]);
            }).catch(notify);
        }) }, _('Initialize access token'));
        return E('div', { 'class': 'cbi-map' }, [
            E('h2', {}, 'QModem Rust'),
            E('p', {}, _('Manage service settings here. Open the dashboard for modem features.')),
            E('p', {}, E('a', { 'class': 'cbi-button cbi-button-action', 'href': dashboardUrl,
                'target': '_blank', 'rel': 'noopener noreferrer' }, _('Open modem dashboard'))),
            E('div', { 'class': 'cbi-section' }, [
                E('h3', {}, _('Service status')), E('p', {}, status),
                E('div', { 'style': 'display:flex;gap:8px;flex-wrap:wrap' }, [
                    button(_('Start'), 'start'), button(_('Stop'), 'stop'), button(_('Restart'), 'restart'),
                    button(_('Enable autostart'), 'enable'), button(_('Disable autostart'), 'disable')
                ])
            ]),
            E('div', { 'class': 'cbi-section' }, [
                E('h3', {}, _('Listener')),
                field(_('Network device'), device, _('Bind incoming connections to this Linux network device, for example br-lan. An unavailable device prevents startup.')),
                field(_('Listen address'), listen, _('IPv4 or IPv6 address. Use 0.0.0.0 or :: for all addresses on the selected device.')),
                field(_('Port'), port, '1–65535'),
                E('p', {}, [tokenState, ' ', createToken])
            ]),
            E('div', { 'class': 'cbi-section' }, [
                E('h3', {}, _('Logging')),
                field(_('Minimum log level'), level, _('Information is recommended. Debug and trace include timings and internal state, without tokens, SMS content or complete AT payloads.')),
                field(_('Log format'), format, _('Logs go to the OpenWrt system log through procd.'))
            ]),
            E('p', {}, _('Settings are saved to TOML and take effect after restarting the service.')),
            E('button', { 'class': 'cbi-button cbi-button-save', 'disabled': readonly, 'click': ui.createHandlerFn(this, function() {
                if (!/^\d+$/.test(port.value) || +port.value < 1 || +port.value > 65535) {
                    notify(new Error(_('Port must be between 1 and 65535'))); return;
                }
                return execute(['set-service', '--listen', listen.value.trim(), '--port', port.value,
                    '--interface', device.value, '--log-level', level.value, '--log-format', format.value]).then(function() {
                    ui.addNotification(null, E('p', {}, _('Saved. Restart the service to apply the settings.')), 'info');
                }).catch(notify);
            }) }, _('Save'))
        ]);
    },
    handleSave: null,
    handleSaveApply: null,
    handleReset: null
});
