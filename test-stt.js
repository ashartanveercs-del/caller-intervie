const WebSocket = require('ws');
const state = require('./cache/app-state.json');

const apiKey = state.assemblyAiKey;

console.log('Testing Assembly AI Key:', apiKey);

const wsUrl = `wss://streaming.assemblyai.com/v3/ws?sample_rate=16000&format_turns=true&speech_model=universal-streaming-english`;

console.log('Connecting to:', wsUrl);

const ws = new WebSocket(wsUrl, {
  headers: { Authorization: apiKey }
});

ws.on('open', () => {
  console.log('SUCCESS! WebSocket connection opened.');
  ws.send(JSON.stringify({ terminate_session: true }));
});

ws.on('message', (data) => {
  console.log('Message from server:', data.toString());
});

ws.on('error', (err) => {
  console.error('WebSocket Error:', err.message);
});

ws.on('unexpected-response', (req, res) => {
  console.error(`Unexpected Response: HTTP ${res.statusCode} ${res.statusMessage}`);
});

ws.on('close', (code, reason) => {
  console.log(`WebSocket closed: ${code} ${reason}`);
  process.exit(0);
});
