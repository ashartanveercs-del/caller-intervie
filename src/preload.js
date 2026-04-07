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

  // Events from main
  onSttStatus: (cb) => ipcRenderer.on('stt-status', (_, d) => cb(d)),
  onSttPartial: (cb) => ipcRenderer.on('stt-partial', (_, d) => cb(d)),
  onSttFinal: (cb) => ipcRenderer.on('stt-final', (_, d) => cb(d)),
  onSttStopped: (cb) => ipcRenderer.on('stt-stopped', (_, d) => cb(d)),
  onSttError: (cb) => ipcRenderer.on('stt-error', (_, d) => cb(d)),
  onShortcut: (cb) => ipcRenderer.on('shortcut', (_, action) => cb(action))
});
