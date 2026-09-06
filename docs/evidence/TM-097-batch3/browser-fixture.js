// Capture the real production Vue bundle with an in-memory, deny-by-default IPC boundary.
// No native bridge exists in this browser session. Never run scans or cleanup commands.
(() => {
  const calls = [];
  const denied = [];
  const values = new Map();
  let callback = 0;
  const callbacks = new Map();
  const disk = { name: 'Fixture volume', mountPoint: '/fixture', totalBytes: 107374182400,
    availableBytes: 64424509440, usedBytes: 42949672960 };
  window.__TM097_FIXTURE__ = { calls, denied };
  window.__TAURI_OS_PLUGIN_INTERNALS__ = { platform: 'linux', version: 'fixture',
    family: 'unix', os_type: 'linux', arch: 'x86_64', eol: '\n', exe_extension: '' };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: 'main' }, currentWebview: { windowLabel: 'main', label: 'main' } },
    transformCallback(fn) { callbacks.set(++callback, fn); return callback; },
    unregisterCallback(id) { callbacks.delete(id); },
    convertFileSrc() { throw new Error('Fixture denies filesystem resource access'); },
    async invoke(cmd, args = {}) {
      calls.push(cmd);
      if (cmd === 'plugin:store|load') return 1;
      if (cmd === 'plugin:store|get') return [values.get(args.key), values.has(args.key)];
      if (cmd === 'plugin:store|set') { values.set(args.key, args.value); return; }
      if (cmd === 'plugin:store|delete') { values.delete(args.key); return; }
      if (cmd === 'plugin:store|save' || cmd === 'plugin:window|show' || cmd === 'plugin:event|unlisten') return;
      if (cmd === 'plugin:event|listen') return ++callback;
      if (cmd === 'plugin:window|is_maximized') return false;
      if (cmd === 'plugin:app|version') return '1.0.6';
      if (cmd === 'plugin:path|resolve_directory') return '/fixture';
      if (cmd === 'plugin:updater|check') return null;
      if (cmd === 'get_app_distribution') return 'installed';
      if (cmd === 'get_or_create_install_id') return 'fixture-only';
      if (cmd === 'get_system_disk') return { ...disk };
      if (cmd === 'list_disks') return [{ ...disk }];
      denied.push(cmd);
      throw new Error('Fixture denies command: ' + cmd);
    },
  };
})();
