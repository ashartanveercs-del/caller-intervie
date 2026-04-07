// ─── STATE ─────────────────────────────────────────────────────────
let transcripts = [];
let messages = [];
let isTranscribing = false;

let micStream = null;
let micRecorder = null;
let micAnalyser = null;
let systemStream = null;
let systemRecorder = null;
let systemAnalyser = null;

let selectedScreenshots = new Set();
let aiLoading = false;
let selectedMicId = 'default';

function arrayBufferToBase64(buffer) {
  let binary = '';
  const bytes = new Uint8Array(buffer);
  for (let i = 0; i < bytes.byteLength; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return window.btoa(binary);
}

// Helper to send blob to main
async function sendAudioBlob(blob, source) {
  if (blob.size < 100) return;
  const buffer = await blob.arrayBuffer();
  const base64 = arrayBufferToBase64(buffer);
  // Using JSON.stringify here is the 100% stable way to pass large strings via contextBridge
  const payload = JSON.stringify({ base64, source });
  window.ghostAPI.transcribeAudio(payload);
}

const chatArea = document.getElementById('chat-area');
const welcomeMsg = document.getElementById('welcome-msg');
const sttPartialText = document.getElementById('stt-partial-text');
const sttMicDot = document.getElementById('stt-mic-dot');
const sttHostDot = document.getElementById('stt-host-dot');
const ssStrip = document.getElementById('screenshot-strip');
const ssThumbs = document.getElementById('ss-thumbs');
const ssCount = document.getElementById('ss-count');

// ─── INIT ──────────────────────────────────────────────────────────
async function init() {
  const state = await window.ghostAPI.getState();
  if (state.groqApiKey) document.getElementById('input-groq-key').value = state.groqApiKey;
  if (state.assemblyAiKey) document.getElementById('input-assembly-key').value = state.assemblyAiKey;
  if (state.language) document.getElementById('select-language').value = state.language;
  if (state.selectedMicId) selectedMicId = state.selectedMicId;

  await updateMicList();

  if (!state.groqApiKey) {
    document.getElementById('settings-modal').style.display = 'flex';
  }
}

async function updateMicList() {
  try {
    // Force permission request to get labels
    await navigator.mediaDevices.getUserMedia({ audio: true });
    
    const devices = await navigator.mediaDevices.enumerateDevices();
    const mics = devices.filter(d => d.kind === 'audioinput');
    const select = document.getElementById('select-audio-input');
    select.innerHTML = '';
    
    if (mics.length === 0) {
      const opt = document.createElement('option');
      opt.textContent = "No Microphones Found";
      select.appendChild(opt);
    }

    mics.forEach(m => {
      const opt = document.createElement('option');
      opt.value = m.deviceId;
      opt.textContent = m.label || `Unknown Mic (${m.deviceId.substring(0,5)})`;
      if (m.deviceId === selectedMicId) opt.selected = true;
      select.appendChild(opt);
    });
  } catch (e) { 
    console.error('Enumerate mics failed:', e); 
    addChatMessage('error-msg', 'Microphone permission denied. Check your Windows/System settings.');
  }
}

init();

// ─── SIMPLE MARKDOWN RENDERER ──────────────────────────────────────
function renderMarkdown(text) {
  let html = text
    // Code blocks
    .replace(/```(\w*)\n([\s\S]*?)```/g, '<pre><code class="lang-$1">$2</code></pre>')
    // Bold
    .replace(/\*\*(.*?)\*\*/g, '<strong>$1</strong>')
    // Inline code
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    // Headers
    .replace(/^### (.*$)/gm, '<h3>$1</h3>')
    .replace(/^## (.*$)/gm, '<h2>$1</h2>')
    .replace(/^# (.*$)/gm, '<h1>$1</h1>')
    // Unordered lists
    .replace(/^- \[ \] (.*$)/gm, '<li>☐ $1</li>')
    .replace(/^- \[x\] (.*$)/gm, '<li>☑ $1</li>')
    .replace(/^[-*] (.*$)/gm, '<li>$1</li>')
    // Paragraphs / line breaks
    .replace(/\n\n/g, '</p><p>')
    .replace(/\n/g, '<br>');

  // Wrap consecutive <li> in <ul>
  html = html.replace(/((?:<li>.*<\/li>\s*)+)/g, '<ul>$1</ul>');
  return `<p>${html}</p>`;
}

// ─── CHAT HELPERS ──────────────────────────────────────────────────
function removeWelcome() {
  if (welcomeMsg) welcomeMsg.style.display = 'none';
}

function addChatMessage(type, content, meta = {}) {
  removeWelcome();
  const div = document.createElement('div');
  div.className = `chat-msg ${type}`;
  const msgIndex = messages.length;

  if (type === 'transcript') {
    const src = meta.source === 'system' ? 'HOST' : 'YOU';
    div.innerHTML = `
      <div class="msg-source">${src}</div>
      <div class="msg-text">${escapeHtml(content)}</div>
      <button class="msg-toggle" data-idx="${msgIndex}" title="Toggle AI context">AI</button>
    `;
    transcripts.push({ source: meta.source, text: content, enabled: true, idx: msgIndex });
  } else if (type === 'ai-response') {
    div.innerHTML = `
      <div class="msg-label">🧠 ${meta.label || 'AI'}</div>
      <div class="msg-text">${renderMarkdown(content)}</div>
    `;
  } else if (type === 'screenshot-msg') {
    div.innerHTML = `📸 Screenshot captured (#${meta.count || '?'})`;
  } else if (type === 'error-msg') {
    div.innerHTML = `⚠️ ${escapeHtml(content)}`;
  } else if (type === 'system-msg') {
    div.innerHTML = content;
  }

  messages.push({ type, content, enabled: true, el: div });
  chatArea.appendChild(div);
  chatArea.scrollTop = chatArea.scrollHeight;
  return div;
}

function addLoadingMessage(label) {
  removeWelcome();
  const div = document.createElement('div');
  div.className = 'chat-msg ai-response loading-msg';
  div.innerHTML = `
    <div class="msg-label">🧠 ${label}</div>
    <div class="loading-dots"><span></span><span></span><span></span></div>
  `;
  chatArea.appendChild(div);
  chatArea.scrollTop = chatArea.scrollHeight;
  return div;
}

function escapeHtml(text) {
  const d = document.createElement('div');
  d.textContent = text;
  return d.innerHTML;
}

// ─── TOGGLE MESSAGE CONTEXT ───────────────────────────────────────
chatArea.addEventListener('click', (e) => {
  if (e.target.classList.contains('msg-toggle')) {
    const idx = parseInt(e.target.dataset.idx);
    const msg = messages[idx];
    if (msg) {
      msg.enabled = !msg.enabled;
      e.target.textContent = msg.enabled ? 'AI' : 'Off';
      e.target.classList.toggle('off', !msg.enabled);
      // Also toggle in transcripts
      const t = transcripts.find(t => t.idx === idx);
      if (t) t.enabled = msg.enabled;
    }
  }
});

// ─── CONTEXT BUILDERS ─────────────────────────────────────────────
function getEnabledTranscripts() {
  return transcripts.filter(t => t.enabled);
}

function getContextMessages() {
  return messages
    .filter(m => m.enabled && m.type === 'ai-response')
    .map(m => ({ role: 'assistant', content: m.content }));
}

function getSelectedScreenshotIndices() {
  if (selectedScreenshots.size > 0) return [...selectedScreenshots];
  return undefined; // let backend use latest
}

// ─── AI ACTIONS ───────────────────────────────────────────────────
async function doAskAI() {
  if (aiLoading) return;
  aiLoading = true;
  setActionButtonsLoading(true);
  const loader = addLoadingMessage('Ask AI — thinking...');

  const result = await window.ghostAPI.askAI({
    transcripts: getEnabledTranscripts(),
    contextMessages: getContextMessages(),
    screenshotIndices: getSelectedScreenshotIndices()
  });

  loader.remove();
  if (result.error) addChatMessage('error-msg', result.error);
  else if (result.response) addChatMessage('ai-response', result.response, { label: 'Ask AI' });
  else addChatMessage('error-msg', 'No response received');

  aiLoading = false;
  setActionButtonsLoading(false);
}

async function doScreenAI() {
  if (aiLoading) return;
  aiLoading = true;
  setActionButtonsLoading(true);
  const loader = addLoadingMessage('Screen AI — analyzing...');

  const result = await window.ghostAPI.screenAI({
    contextMessages: getContextMessages(),
    screenshotIndices: getSelectedScreenshotIndices()
  });

  loader.remove();
  if (result.error) addChatMessage('error-msg', result.error);
  else if (result.response) addChatMessage('ai-response', result.response, { label: 'Screen AI' });
  else addChatMessage('error-msg', 'No response received');

  aiLoading = false;
  setActionButtonsLoading(false);
}

async function doSuggest() {
  if (aiLoading) return;
  aiLoading = true;
  setActionButtonsLoading(true);
  const loader = addLoadingMessage('Suggest — thinking...');

  const result = await window.ghostAPI.suggest({
    transcripts: getEnabledTranscripts(),
    contextMessages: getContextMessages()
  });

  loader.remove();
  if (result.error) addChatMessage('error-msg', result.error);
  else if (result.response) addChatMessage('ai-response', result.response, { label: 'Suggest' });
  else addChatMessage('error-msg', 'No response received');

  aiLoading = false;
  setActionButtonsLoading(false);
}

async function doNotes() {
  if (aiLoading) return;
  aiLoading = true;
  setActionButtonsLoading(true);
  const loader = addLoadingMessage('Notes — generating...');

  const result = await window.ghostAPI.notes({
    transcripts: getEnabledTranscripts(),
    contextMessages: getContextMessages()
  });

  loader.remove();
  if (result.error) addChatMessage('error-msg', result.error);
  else if (result.response) addChatMessage('ai-response', result.response, { label: 'Notes' });
  else addChatMessage('error-msg', 'No response received');

  aiLoading = false;
  setActionButtonsLoading(false);
}

function setActionButtonsLoading(loading) {
  document.querySelectorAll('.action-btn').forEach(b => {
    b.classList.toggle('loading', loading);
  });
}

// ─── SCREENSHOT ───────────────────────────────────────────────────
async function doScreenshot() {
  const result = await window.ghostAPI.takeScreenshot();
  if (result && result.success) {
    addChatMessage('screenshot-msg', '', { count: result.count });
    updateScreenshotStrip();
  } else {
    addChatMessage('error-msg', result?.error || 'Screenshot failed');
  }
}

async function updateScreenshotStrip() {
  const shots = await window.ghostAPI.getScreenshots();
  if (shots.length === 0) {
    ssStrip.style.display = 'none';
    return;
  }
  ssStrip.style.display = 'flex';
  ssCount.textContent = shots.length;
  ssThumbs.innerHTML = '';
  shots.forEach(s => {
    const img = document.createElement('img');
    img.className = 'ss-thumb' + (selectedScreenshots.has(s.index) ? ' selected' : '');
    img.src = s.preview;
    img.dataset.index = s.index;
    img.addEventListener('click', () => {
      if (selectedScreenshots.has(s.index)) selectedScreenshots.delete(s.index);
      else selectedScreenshots.add(s.index);
      img.classList.toggle('selected');
    });
    ssThumbs.appendChild(img);
  });
  // Auto scroll to end
  ssThumbs.scrollLeft = ssThumbs.scrollWidth;
}

// ─── TRANSCRIPTION ────────────────────────────────────────────────
async function toggleTranscription() {
  if (isTranscribing) {
    stopTranscription();
  } else {
    startTranscription();
  }
}

async function startTranscription() {
  isTranscribing = true;
  document.getElementById('btn-transcribe').classList.add('active');
  addChatMessage('system-msg', '🎙️ Transcription started (Groq Whisper Stable)');

  // Start mic
  try {
    micStream = await navigator.mediaDevices.getUserMedia({
      audio: { 
        deviceId: selectedMicId !== 'default' ? { exact: selectedMicId } : undefined,
        channelCount: 1, 
        sampleRate: 16000, 
        echoCancellation: true, 
        noiseSuppression: true 
      }
    });

    // VU Meter logic
    micVuContext = new AudioContext();
    const sourceNode = micVuContext.createMediaStreamSource(micStream);
    micAnalyser = micVuContext.createAnalyser();
    micAnalyser.fftSize = 256;
    sourceNode.connect(micAnalyser);

    const vuBar = document.getElementById('vu-bar');
    const updateVU = () => {
      if (!isTranscribing || !micAnalyser) {
        vuBar.style.width = '0%';
        return;
      }
      const data = new Uint8Array(micAnalyser.frequencyBinCount);
      micAnalyser.getByteFrequencyData(data);
      let sum = 0;
      for (const v of data) sum += v;
      const average = sum / data.length;
      const volume = Math.min(100, Math.floor((average / 128) * 100));
      vuBar.style.width = volume + '%';
      requestAnimationFrame(updateVU);
    };
    updateVU();
    
    // Cyclic recording: Whisper requires a fresh file header for every chunk
    const recordMic = () => {
      if (!isTranscribing) return;
      const r = new MediaRecorder(micStream, { mimeType: 'audio/webm' });
      r.ondataavailable = (e) => sendAudioBlob(e.data, 'mic');
      r.onstop = () => { if (isTranscribing) setTimeout(recordMic, 100); };
      r.start();
      setTimeout(() => { if (r.state === 'recording') r.stop(); }, 4000);
      micRecorder = r;
    };
    recordMic();

    window.ghostAPI.startSTT('mic');
  } catch (err) {
    addChatMessage('error-msg', 'Mic access failed: ' + err.message);
  }

  // System Audio for capturing the Interviewer
  try {
    const sources = await window.ghostAPI.getDesktopSources();
    if (sources.length > 0) {
      // Pick first screen with audio
      const sysStream = await navigator.mediaDevices.getUserMedia({
        audio: {
          mandatory: {
            chromeMediaSource: 'desktop',
            chromeMediaSourceId: sources[0].id
          }
        },
        video: {
          mandatory: {
            chromeMediaSource: 'desktop',
            chromeMediaSourceId: sources[0].id
          }
        }
      });
      // We only need the audio track
      systemStream = new MediaStream(sysStream.getAudioTracks());
      
      const sysSource = new AudioContext().createMediaStreamSource(systemStream);
      systemAnalyser = sysSource.context.createAnalyser();
      systemAnalyser.fftSize = 256;
      sysSource.connect(systemAnalyser);

      const vuBarHost = document.getElementById('vu-bar-host');
      const updateSystemVU = () => {
        if (!isTranscribing || !systemAnalyser) {
          vuBarHost.style.width = '0%';
          return;
        }
        const data = new Uint8Array(systemAnalyser.frequencyBinCount);
        systemAnalyser.getByteFrequencyData(data);
        let sum = 0;
        for (const v of data) sum += v;
        const volume = Math.min(100, Math.floor((sum / data.length / 128) * 100));
        vuBarHost.style.width = volume + '%';
        requestAnimationFrame(updateSystemVU);
      };
      updateSystemVU();

      const recordSystem = () => {
        if (!isTranscribing) return;
        const r = new MediaRecorder(systemStream, { mimeType: 'audio/webm' });
        r.ondataavailable = (e) => sendAudioBlob(e.data, 'system');
        r.onstop = () => { if (isTranscribing) setTimeout(recordSystem, 100); };
        r.start();
        setTimeout(() => { if (r.state === 'recording') r.stop(); }, 4000);
        systemRecorder = r;
      };
      recordSystem();

      window.ghostAPI.startSTT('system');
    }
  } catch (err) {
    console.log('System audio not available:', err.message);
  }
}

function stopTranscription() {
  isTranscribing = false;
  document.getElementById('btn-transcribe').classList.remove('active');

  if (micRecorder && micRecorder.state !== 'inactive') micRecorder.stop();
  if (micStream) micStream.getTracks().forEach(t => t.stop());
  micRecorder = null;
  micStream = null;

  if (systemRecorder && systemRecorder.state !== 'inactive') systemRecorder.stop();
  if (systemStream) systemStream.getTracks().forEach(t => t.stop());
  systemRecorder = null;
  systemStream = null;

  window.ghostAPI.stopSTT('mic');
  window.ghostAPI.stopSTT('system');
  addChatMessage('system-msg', '⏹️ Transcription stopped');
}

async function doClear() {
  stopTranscription();
  await window.ghostAPI.clearChat();
  await window.ghostAPI.clearScreenshots();
  transcripts = [];
  messages = [];
  selectedScreenshots.clear();
  chatArea.innerHTML = '';
  if (welcomeMsg) {
    welcomeMsg.style.display = 'flex';
    chatArea.appendChild(welcomeMsg);
  }
  ssStrip.style.display = 'none';
  ssThumbs.innerHTML = '';
}

// ─── STT EVENTS ───────────────────────────────────────────────────
window.ghostAPI.onSttStatus(({ source, status }) => {
  const dot = source === 'system' ? sttHostDot : sttMicDot;
  if (status === 'listening') dot.classList.add('active');
  else dot.classList.remove('active');
});

window.ghostAPI.onSttPartial(({ source, text }) => {
  sttPartialText.textContent = text.substring(0, 80);
});

window.ghostAPI.onSttFinal(({ source, text }) => {
  if (text && text.trim()) {
    addChatMessage('transcript', text.trim(), { source });
    sttPartialText.textContent = '';
  }
});

window.ghostAPI.onSttStopped(({ source }) => {
  const dot = source === 'system' ? sttHostDot : sttMicDot;
  dot.classList.remove('active');
});

window.ghostAPI.onSttError(({ source, error }) => {
  addChatMessage('error-msg', `STT Error (${source}): ${error}`);
});

// ─── KEYBOARD SHORTCUTS ───────────────────────────────────────────
window.ghostAPI.onShortcut((action) => {
  switch (action) {
    case 'toggle-transcription': toggleTranscription(); break;
    case 'take-screenshot': doScreenshot(); break;
    case 'ask-ai': doAskAI(); break;
    case 'screen-ai': doScreenAI(); break;
    case 'suggest': doSuggest(); break;
    case 'notes': doNotes(); break;
    case 'clear-chat': doClear(); break;
  }
});

// ─── BUTTON HANDLERS ──────────────────────────────────────────────
document.getElementById('btn-suggest').addEventListener('click', doSuggest);
document.getElementById('btn-ask').addEventListener('click', doAskAI);
document.getElementById('btn-screen').addEventListener('click', doScreenAI);
document.getElementById('btn-notes').addEventListener('click', doNotes);
document.getElementById('btn-transcribe').addEventListener('click', toggleTranscription);
document.getElementById('btn-screenshot').addEventListener('click', doScreenshot);
document.getElementById('btn-clear').addEventListener('click', doClear);
document.getElementById('btn-hide').addEventListener('click', () => window.ghostAPI.toggleVisibility());

document.getElementById('opacity-slider').addEventListener('input', (e) => {
  window.ghostAPI.setOpacity(parseInt(e.target.value) / 100);
});

// ─── SETTINGS ─────────────────────────────────────────────────────
document.getElementById('btn-settings').addEventListener('click', async () => {
  await updateMicList();
  document.getElementById('settings-modal').style.display = 'flex';
});

document.getElementById('settings-close').addEventListener('click', () => {
  document.getElementById('settings-modal').style.display = 'none';
});

document.getElementById('settings-modal').addEventListener('click', (e) => {
  if (e.target.id === 'settings-modal') {
    document.getElementById('settings-modal').style.display = 'none';
  }
});

document.getElementById('btn-save-settings').addEventListener('click', async () => {
  const settings = {
    groqApiKey: document.getElementById('input-groq-key').value.trim(),
    selectedMicId: document.getElementById('select-audio-input').value,
    language: document.getElementById('select-language').value
  };

  const result = await window.ghostAPI.saveSettings(settings);
  if (result.success) {
    selectedMicId = settings.selectedMicId;
    document.getElementById('settings-modal').style.display = 'none';
    addChatMessage('system-msg', '✅ Settings saved');
  }
});
