const path = require('path');
const { app, BrowserWindow, globalShortcut, ipcMain, desktopCapturer, screen, nativeImage } = require('electron');
require('dotenv').config({ path: path.join(__dirname, '..', '.env') });

const { createGroqService } = require('./groq-service');
const { createDeepgramService } = require('./deepgram-service');
const { loadState, saveState } = require('./state');
const fs = require('fs');
const os = require('os');

console.log('Debug: Main Process Booted');

// Prevent any unhandled errors (like AssemblyAI connection drops) from crashing the stealth app
process.on('uncaughtException', (err) => {
  console.error('Ignored global crash:', err);
});
process.on('unhandledRejection', (reason) => {
  console.error('Ignored promise rejection:', reason);
});

const HIDE_FROM_SCREEN_CAPTURE = process.env.HIDE_FROM_SCREEN_CAPTURE !== 'false';
const START_HIDDEN = process.env.START_HIDDEN === 'true';
const MAX_SCREENSHOTS = parseInt(process.env.MAX_SCREENSHOTS || '20', 10);

let mainWindow = null;
let groqService = null;
let appState = {};
let screenshots = [];
let chatHistory = [];

// Transcription state
let isStreamingMic = false;
let isStreamingSystem = false;
let dgMic = null;
let dgSystem = null;

const DEEPGRAM_KEY = process.env.DEEPGRAM_API_KEY || '';

function sendToRenderer(channel, data) {
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send(channel, data);
  }
}

function initGroqService() {
  const apiKey = appState.groqApiKey || process.env.GROQ_API_KEY || '';
  if (!apiKey) return;
  groqService = createGroqService(apiKey);
}

function createWindow() {
  const { width, height } = screen.getPrimaryDisplay().workAreaSize;
  const winWidth = 420;
  const winHeight = 680;

  mainWindow = new BrowserWindow({
    width: winWidth,
    height: winHeight,
    minWidth: 320,
    minHeight: 400,
    x: width - winWidth - 20,
    y: 40,
    webPreferences: {
      nodeIntegration: false,
      contextIsolation: true,
      preload: path.join(__dirname, 'preload.js'),
      backgroundThrottling: false,
      sandbox: true
    },
    frame: false,
    transparent: true,
    alwaysOnTop: true,
    skipTaskbar: true,
    resizable: true,
    minimizable: false,
    maximizable: false,
    closable: false,
    focusable: true,
    show: false,
    type: 'toolbar',
    hasShadow: false,
    thickFrame: false,
    titleBarStyle: 'hidden',
    backgroundColor: '#00000000'
  });

  mainWindow.loadFile(path.join(__dirname, 'renderer', 'index.html'));

  // Permissions for microphone
  mainWindow.webContents.session.setPermissionRequestHandler((wc, perm, cb) => {
    cb(perm === 'microphone' || perm === 'media');
  });
  mainWindow.webContents.session.setPermissionCheckHandler((wc, perm) => {
    return perm === 'microphone' || perm === 'media';
  });

  // Windows stealth
  if (process.platform === 'win32') {
    mainWindow.setSkipTaskbar(true);
    mainWindow.setAlwaysOnTop(true, 'pop-up-menu');
    mainWindow.setAppDetails({ appId: 'SystemProcess', appIconPath: '', relaunchCommand: '', relaunchDisplayName: '' });
  }

  mainWindow.setContentProtection(HIDE_FROM_SCREEN_CAPTURE);

  mainWindow.webContents.on('did-finish-load', () => {
    mainWindow.webContents.executeJavaScript(`
      document.body.style.background = 'transparent';
      document.body.style.visibility = 'visible';
    `).then(() => {
      if (!START_HIDDEN) mainWindow.showInactive();
    });
  });

  if (process.env.NODE_ENV === 'development') {
    mainWindow.webContents.openDevTools();
  }
}

// ─── TRANSCRIPTION ───────────────────────────────────────────────────
async function initDeepgram(source) {
  const onTranscript = (res) => {
    if (res.isFinal) {
      console.log(`STT [${source}][FINAL]: ${res.text}`);
      sendToRenderer('stt-final', { 
        source, 
        text: res.text, 
        isQuestion: res.isQuestion,
        speechFinal: res.speechFinal
      });
    } else {
      console.log(`STT [${source}][Partial]: ${res.text}...`);
      sendToRenderer('stt-partial', { source, text: res.text });
    }
  };

  const onError = (errorMsg) => {
    console.error(`Deepgram [${source}] fatal: ${errorMsg}`);
    sendToRenderer('stt-error', { source, error: errorMsg });
  };

  if (source === 'mic') {
    if (!dgMic) dgMic = createDeepgramService(DEEPGRAM_KEY, onTranscript, onError);
    await dgMic.connect();
  } else {
    if (!dgSystem) dgSystem = createDeepgramService(DEEPGRAM_KEY, onTranscript, onError);
    await dgSystem.connect();
  }
}

// ─── SCREENSHOT ──────────────────────────────────────────────────────
async function takeScreenshot() {
  try {
    // Briefly hide window
    if (mainWindow && !mainWindow.isDestroyed()) {
      mainWindow.hide();
      await new Promise(r => setTimeout(r, 300));
    }

    const sources = await desktopCapturer.getSources({
      types: ['screen'],
      thumbnailSize: { width: 1920, height: 1080 }
    });

    if (mainWindow && !mainWindow.isDestroyed()) {
      mainWindow.showInactive();
    }

    if (sources.length === 0) return null;
    const img = sources[0].thumbnail;
    const base64 = img.toDataURL().replace(/^data:image\/png;base64,/, '');

    screenshots.push({ base64, timestamp: Date.now() });
    if (screenshots.length > MAX_SCREENSHOTS) screenshots.shift();

    return { success: true, count: screenshots.length, preview: img.toDataURL() };
  } catch (err) {
    if (mainWindow && !mainWindow.isDestroyed()) mainWindow.showInactive();
    return { success: false, error: err.message };
  }
}

// ─── IPC HANDLERS ────────────────────────────────────────────────────
function registerIPC() {
  ipcMain.handle('get-state', () => appState);

  ipcMain.handle('save-settings', (_, settings) => {
    if (!settings || typeof settings !== 'object') return { success: false, error: 'Invalid settings' };
    const allowed = ['groqApiKey', 'selectedMicId', 'language'];
    const filtered = {};
    for (const key of allowed) {
      if (settings[key] !== undefined) filtered[key] = String(settings[key]);
    }
    appState = { ...appState, ...filtered };
    saveState(appState);
    initGroqService();
    return { success: true };
  });

  ipcMain.handle('take-screenshot', async () => takeScreenshot());

  ipcMain.handle('get-screenshots', () => screenshots.map((s, i) => ({
    index: i, preview: `data:image/png;base64,${s.base64}`, timestamp: s.timestamp
  })));

  ipcMain.handle('clear-screenshots', () => { screenshots = []; return { success: true }; });

  ipcMain.handle('start-stt', async (_, { source }) => {
    if (source === 'system') isStreamingSystem = true; else isStreamingMic = true;
    await initDeepgram(source);
    sendToRenderer('stt-status', { source, status: 'listening' });
    return { success: true };
  });

  ipcMain.handle('stop-stt', (_, { source }) => {
    if (source === 'system') {
      isStreamingSystem = false;
      if (dgSystem) { dgSystem.disconnect(); dgSystem = null; }
    } else {
      isStreamingMic = false;
      if (dgMic) { dgMic.disconnect(); dgMic = null; }
    }
    sendToRenderer('stt-stopped', { source });
    return { success: true };
  });

  ipcMain.on('transcribe-audio', (event, jsonPayload) => {
    try {
      const { base64, source } = JSON.parse(jsonPayload);
      const buffer = Buffer.from(base64, 'base64');
      if (source === 'mic' && dgMic) dgMic.sendAudio(buffer);
      else if (source === 'system' && dgSystem) dgSystem.sendAudio(buffer);
    } catch (err) {
      console.error('Deepgram IPC error:', err.message);
    }
  });

  ipcMain.handle('get-desktop-sources', async () => {
    const sources = await desktopCapturer.getSources({ types: ['screen'] });
    return sources.map(s => ({ id: s.id, name: s.name }));
  });

  // AI actions
  ipcMain.handle('ask-ai', async (_, { transcripts, contextMessages, screenshotIndices }) => {
    if (!groqService) return { error: 'Groq API key not set. Open Settings.' };
    const imgs = (screenshotIndices || []).map(i => screenshots[i]?.base64).filter(Boolean);
    return await groqService.askAI(transcripts, contextMessages, imgs);
  });

  ipcMain.handle('screen-ai', async (_, { contextMessages, screenshotIndices }) => {
    if (!groqService) return { error: 'Groq API key not set. Open Settings.' };
    const imgs = (screenshotIndices || []).map(i => screenshots[i]?.base64).filter(Boolean);
    if (imgs.length === 0 && screenshots.length > 0) {
      imgs.push(screenshots[screenshots.length - 1].base64);
    }
    return await groqService.screenAI(contextMessages, imgs);
  });

  ipcMain.handle('suggest', async (_, { transcripts, contextMessages }) => {
    if (!groqService) return { error: 'Groq API key not set. Open Settings.' };
    return await groqService.suggest(transcripts, contextMessages);
  });

  ipcMain.handle('notes', async (_, { transcripts, contextMessages }) => {
    if (!groqService) return { error: 'Groq API key not set. Open Settings.' };
    return await groqService.notes(transcripts, contextMessages);
  });

  ipcMain.handle('clear-chat', () => {
    chatHistory = [];
    screenshots = [];
    if (groqService) groqService.clearHistory();
    return { success: true };
  });

  // Window control
  ipcMain.handle('toggle-visibility', () => {
    if (mainWindow.isVisible()) mainWindow.hide();
    else mainWindow.showInactive();
  });

  ipcMain.handle('set-opacity', (_, opacity) => {
    const val = parseFloat(opacity);
    if (isNaN(val)) return;
    mainWindow.setOpacity(Math.max(0.1, Math.min(1, val)));
  });
}

// ─── SHORTCUTS ────────────────────────────────────────────────────────
function registerShortcuts() {
  const shortcuts = {
    'Alt+Shift+T': () => sendToRenderer('shortcut', 'toggle-transcription'),
    'Alt+Shift+S': () => sendToRenderer('shortcut', 'take-screenshot'),
    'Alt+Shift+A': () => sendToRenderer('shortcut', 'ask-ai'),
    'Alt+Shift+E': () => sendToRenderer('shortcut', 'screen-ai'),
    'Alt+Shift+G': () => sendToRenderer('shortcut', 'suggest'),
    'Alt+Shift+N': () => sendToRenderer('shortcut', 'notes'),
    'Alt+Shift+C': () => sendToRenderer('shortcut', 'clear-chat'),
    'Alt+Shift+X': () => {
      if (mainWindow) {
        if (mainWindow.isVisible()) mainWindow.hide();
        else mainWindow.showInactive();
      }
    },
    'Alt+Shift+H': () => {
      if (mainWindow) {
        if (mainWindow.isVisible()) mainWindow.hide();
        else mainWindow.showInactive();
      }
    },
    'Alt+Shift+Left': () => {
      if (mainWindow) {
        const bounds = mainWindow.getBounds();
        mainWindow.setPosition(20, bounds.y);
      }
    },
    'Alt+Shift+Right': () => {
      if (mainWindow) {
        const { width } = screen.getPrimaryDisplay().workAreaSize;
        const bounds = mainWindow.getBounds();
        mainWindow.setPosition(width - bounds.width - 20, bounds.y);
      }
    },
    'Alt+Shift+Up': () => {
      if (mainWindow) {
        const bounds = mainWindow.getBounds();
        mainWindow.setPosition(bounds.x, 20);
      }
    },
    'Alt+Shift+Down': () => {
      if (mainWindow) {
        const { height } = screen.getPrimaryDisplay().workAreaSize;
        const bounds = mainWindow.getBounds();
        mainWindow.setPosition(bounds.x, height - bounds.height - 20);
      }
    }
  };

  Object.entries(shortcuts).forEach(([accel, fn]) => {
    globalShortcut.register(accel, fn);
  });
}

// ─── APP LIFECYCLE ────────────────────────────────────────────────────
app.whenReady().then(() => {
  appState = loadState();
  initGroqService();
  createWindow();
  registerIPC();
  registerShortcuts();
});

app.on('will-quit', () => {
  globalShortcut.unregisterAll();
});

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit();
});
