'use strict';
'require view';
'require rpc';
'require fs';
'require ui';
'require poll';

var serviceName = 'qmodem-rust';
var list = rpc.declare({ object: 'rc', method: 'list', expect: { '': {} } });
var control = rpc.declare({ object: 'rc', method: 'init', params: ['name', 'action'] });

var procd = rpc.declare({ object: 'service', method: 'list', params: ['name'], expect: { '': {} } });
function statusData() { return Promise.all([list(), procd(serviceName)]); }

function execute(args) {
    return fs.exec('/usr/sbin/qmodemd', ['--config', '/etc/qmodem-rust.toml'].concat(args)).then(function(result) {
        if (result.code !== 0) throw new Error(result.stderr || _('Command failed'));
        return JSON.parse(result.stdout);
    });
}
function notify(error) {
    ui.addNotification(null, E('p', {}, error.message || String(error)), 'error');
}

return view.extend({
    load: function() {
        return Promise.all([execute(['service-info']), statusData()]);
    },
    render: function(data) {
        var cfg = data[0];
        var status = E('span');
        var listen = E('input', { 'class': 'cbi-input-text', 'type': 'text', 'value': cfg.listen, 'aria-label': _('Listen address') });
        var port = E('input', { 'class': 'cbi-input-text', 'type': 'number', 'min': '1', 'max': '65535', 'value': cfg.port, 'aria-label': _('Port') });
        var update = function(services) {
            var entry = services[0][serviceName] || {};
            var instances = (services[1][serviceName] || {}).instances || {};
            var running = Object.keys(instances).some(function(key) { return instances[key].running; });
            status.textContent = (running ? _('Running') : _('Stopped')) + ' / ' + (entry.enabled ? _('Autostart enabled') : _('Autostart disabled'));
        };
        update(data[1]);
        poll.add(function() { return statusData().then(update).catch(notify); }, 5);
        var button = function(label, action) {
            return E('button', { 'class': 'cbi-button cbi-button-action', 'disabled': !L.hasViewPermission(), 'click': ui.createHandlerFn(this, function() {
                return control(serviceName, action).then(function(result) {
                    if (result) throw new Error(_('Service action failed'));
                    return statusData().then(update);
                }).catch(notify);
            }) }, label);
        }.bind(this);
        return E('div', { 'class': 'cbi-map' }, [
            E('h2', {}, 'QModem Rust'),
            E('p', {}, _('Initial development build: service control is available; modem management and the standalone dashboard are not implemented yet.')),
            E('div', { 'class': 'cbi-section' }, [
                E('h3', {}, _('Service status')),
                E('p', {}, status),
                E('div', { 'style': 'display:flex;gap:8px;flex-wrap:wrap' }, [
                    button(_('Start'), 'start'), button(_('Stop'), 'stop'), button(_('Restart'), 'restart'),
                    button(_('Enable autostart'), 'enable'), button(_('Disable autostart'), 'disable')
                ])
            ]),
            E('div', { 'class': 'cbi-section' }, [
                E('h3', {}, _('Basic settings')),
                E('p', {}, _('This initial build only accepts loopback addresses. Settings are stored in TOML; restart the service to apply them.')),
                E('label', {}, [_('Listen address'), listen]),
                E('label', {}, [_('Port'), port]),
                E('p', {}, E('button', { 'class': 'cbi-button cbi-button-save', 'disabled': !L.hasViewPermission(), 'click': ui.createHandlerFn(this, function() {
                    if (!/^\d+$/.test(port.value) || +port.value < 1 || +port.value > 65535) {
                        notify(new Error(_('Port must be between 1 and 65535')));
                        return;
                    }
                    return execute(['set-service', '--listen', listen.value, '--port', port.value]).then(function() {
                        ui.addNotification(null, E('p', {}, _('Saved. Restart the service to apply the settings.')), 'info');
                    }).catch(notify);
                }) }, _('Save')))
            ])
        ]);
    },
    handleSave: null,
    handleSaveApply: null,
    handleReset: null
});
