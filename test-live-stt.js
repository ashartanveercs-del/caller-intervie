const { spawn } = require('child_process');
const WebSocket = require('ws');

const API_KEY = 'df8ecfe42f5ed24ac84a473b3f30e032ab439a76';
// Using your internal microphone found via ffmpeg -list_devices
const MIC_NAME = "audio=Internal Microphone (Conexant ISST Audio)"; 

const url = 'wss://api.deepgram.com/v1/listen?model=nova-2&smart_format=true&encoding=linear16&sample_rate=16000&channels=1';

console.log('Connecting to Deepgram...');
const dgSocket = new WebSocket(url, { headers: { Authorization: `Token ${API_KEY}` } });

dgSocket.on('open', () => {
    console.log('\n✅ DEEPGRAM CONNECTED!');
    console.log('🎤 LISTENING TO: ' + MIC_NAME);
    console.log('==================================================');
    console.log('SPEAK NOW - Transcription will appear below:');
    console.log('==================================================\n');
    
    // Start FFmpeg to capture mic and output raw PCM to stdout
    const ffmpeg = spawn('ffmpeg', [
        '-f', 'dshow',
        '-i', MIC_NAME,
        '-f', 's16le',
        '-ar', '16000',
        '-ac', '1',
        '-'
    ]);

    ffmpeg.stdout.on('data', (chunk) => {
        if (dgSocket.readyState === WebSocket.OPEN) {
            dgSocket.send(chunk);
        }
    });

    ffmpeg.on('error', (err) => {
        console.error('FFmpeg Error:', err.message);
    });
    
    process.on('SIGINT', () => {
        ffmpeg.kill();
        dgSocket.close();
        process.exit();
    });
});

dgSocket.on('message', (data) => {
    try {
        const res = JSON.parse(data);
        const transcript = res.channel?.alternatives[0]?.transcript;
        if (transcript && res.is_final) {
            process.stdout.write(`\r[FINAL]: ${transcript}\n`);
        } else if (transcript) {
            process.stdout.write(`\r[Partial]: ${transcript}...`);
        }
    } catch (e) {
        // Ignore non-json or metadata
    }
});

dgSocket.on('error', (err) => console.error('\n❌ Deepgram Error:', err.message));
dgSocket.on('close', () => console.log('\nℹ️ Deepgram Connection Closed'));
