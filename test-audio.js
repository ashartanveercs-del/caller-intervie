const WebSocket = require('ws');
const state = require('./cache/app-state.json');

const apiKey = state.assemblyAiKey;
const wsUrl = `wss://streaming.assemblyai.com/v3/ws?sample_rate=16000&format_turns=true&speech_model=universal-streaming-english`;

console.log('Connecting to Assembly AI:', wsUrl);

const ws = new WebSocket(wsUrl, { headers: { Authorization: apiKey } });

ws.on('open', () => {
  console.log('Opened! Sending fake 16kHz audio...');
  
  // Send fake sine wave 16-bit PCM for 2 seconds
  const sampleRate = 16000;
  let t = 0;
  const interval = setInterval(() => {
    const buffer = Buffer.alloc(4096 * 2);
    for (let i = 0; i < 4096; i++) {
        const val = Math.floor(Math.sin(t * 440 * Math.PI * 2) * 10000);
        buffer.writeInt16LE(val, i * 2);
        t += 1/sampleRate;
    }
    ws.send(buffer);
  }, 250);

  setTimeout(() => {
    clearInterval(interval);
    ws.close();
    console.log('Done sending audio, terminating session.')
  }, 3000);
});

ws.on('message', (msg) => {
  console.log('RCV:', msg.toString());
});

ws.on('error', console.error);
ws.on('close', console.log);
