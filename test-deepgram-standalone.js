const WebSocket = require('ws');

const API_KEY = 'df8ecfe42f5ed24ac84a473b3f30e032ab439a76';
// Using nova-2 as it is the most stable widely available model
const url = 'wss://api.deepgram.com/v1/listen?model=nova-2&smart_format=true&encoding=linear16&sample_rate=16000';

console.log('Connecting to Deepgram...');

const socket = new WebSocket(url, {
  headers: {
    Authorization: `Token ${API_KEY}`
  }
});

socket.on('open', () => {
  console.log('✅ Deepgram Connection Opened Successfully!');
  console.log('Key is valid and model is accepted.');
  // Send a small dummy buffer to verify data flow
  socket.send(Buffer.alloc(100));
});

socket.on('message', (data) => {
  try {
    const res = JSON.parse(data);
    console.log('STT Response:', JSON.stringify(res, null, 2));
  } catch (e) {
    console.log('Raw Message received:', data.toString());
  }
});

socket.on('error', (err) => {
  console.error('❌ Connection Error:', err.message);
});

socket.on('close', (code, reason) => {
  console.log(`Disconnected: Code ${code}, Reason: ${reason || 'No reason provided'}`);
});

setTimeout(() => {
  console.log('Test complete. Closing...');
  socket.close();
  process.exit(0);
}, 5000);
