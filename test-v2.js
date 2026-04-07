const WebSocket = require('ws');
const state = require('./cache/app-state.json');

const apiKey = state.assemblyAiKey;
const wsUrl = `wss://api.assemblyai.com/v2/realtime/ws?sample_rate=16000`;

console.log('Connecting v2 to Assembly AI:', wsUrl);
console.log('Using Key:', apiKey);

const ws = new WebSocket(wsUrl, { headers: { Authorization: apiKey } });

ws.on('open', () => {
  console.log('Opened! Sending base64 audio...');
  
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
    ws.send(JSON.stringify({ audio_data: buffer.toString('base64') }));
  }, 250);

  setTimeout(() => {
    clearInterval(interval);
    ws.send(JSON.stringify({ terminate_session: true }));
    console.log('Done sending audio, terminating session.')
  }, 3000);
});

ws.on('message', (msg) => {
  console.log('RCV:', msg.toString());
});

ws.on('error', console.error);
ws.on('unexpected-response', (req, res) => {
  console.error('UNEXPECTED HTTP:', res.statusCode);
  req.destroy();
  process.exit(1);
})
ws.on('close', console.log);
