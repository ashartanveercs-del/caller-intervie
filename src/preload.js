const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('ghostAPI', {
  // Settings
  getState: () => ipcRenderer.invoke('get-state'),
  saveSettings: (s) => ipcRenderer.invoke('save-settings', s),

  // Screenshots
  takeScreenshot: () => ipcRenderer.invoke('take-screenshot'),
  getScreenshots: () => ipcRenderer.invoke('get-screenshots'),
  clearScreenshots: () => ipcRenderer.invoke('clear-screenshots'),

  // Transcription
  startSTT: (source) => ipcRenderer.invoke('start-stt', { source }),
  stopSTT: (source) => ipcRenderer.invoke('stop-stt', { source }),
  transcribeAudio: (jsonPayload) => ipcRenderer.send('transcribe-audio', jsonPayload),
  getDesktopSources: () => ipcRenderer.invoke('get-desktop-sources'),

  // AI actions
  askAI: (data) => ipcRenderer.invoke('ask-ai', data),
  screenAI: (data) => ipcRenderer.invoke('screen-ai', data),
  suggest: (data) => ipcRenderer.invoke('suggest', data),
  notes: (data) => ipcRenderer.invoke('notes', data),

  // Chat
  clearChat: () => ipcRenderer.invoke('clear-chat'),

  // Window
  toggleVisibility: () => ipcRenderer.invoke('toggle-visibility'),
  setOpacity: (v) => ipcRenderer.invoke('set-opacity', v),

  // Events from main (with cleanup support)
  onSttStatus: (cb) => {
    const handler = (_, d) => cb(d);
    ipcRenderer.on('stt-status', handler);
    return () => ipcRenderer.removeListener('stt-status', handler);
  },
  onSttPartial: (cb) => {
    const handler = (_, d) => cb(d);
    ipcRenderer.on('stt-partial', handler);
    return () => ipcRenderer.removeListener('stt-partial', handler);
  },
  onSttFinal: (cb) => {
    const handler = (_, d) => cb(d);
    ipcRenderer.on('stt-final', handler);
    return () => ipcRenderer.removeListener('stt-final', handler);
  },
  onSttStopped: (cb) => {
    const handler = (_, d) => cb(d);
    ipcRenderer.on('stt-stopped', handler);
    return () => ipcRenderer.removeListener('stt-stopped', handler);
  },
  onSttError: (cb) => {
    const handler = (_, d) => cb(d);
    ipcRenderer.on('stt-error', handler);
    return () => ipcRenderer.removeListener('stt-error', handler);
  },
  onShortcut: (cb) => {
    const handler = (_, action) => cb(action);
    ipcRenderer.on('shortcut', handler);
    return () => ipcRenderer.removeListener('shortcut', handler);
  }
});
