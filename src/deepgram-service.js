const WebSocket = require('ws');

function createDeepgramService(apiKey, onTranscript, onError) {
  let dgConnection = null;
  let keepAliveInterval = null;
  let pendingBuffer = [];
  let isActive = false;
  let reconnectTimer = null;
  let reconnectAttempts = 0;
  const MAX_RECONNECTS = 5;
  let hasEverReceivedData = false;

  function connect() {
    return new Promise((resolve) => {
      if (dgConnection && dgConnection.readyState === WebSocket.OPEN) {
        resolve();
        return;
      }
      if (dgConnection && dgConnection.readyState === WebSocket.CONNECTING) {
        dgConnection.once('open', () => resolve());
        dgConnection.once('error', () => resolve());
        return;
      }

      isActive = true;

      const url = 'wss://api.deepgram.com/v1/listen?model=nova-2&encoding=linear16&sample_rate=16000&channels=1&smart_format=true&filler_words=true&endpointing=300&interim_results=true&punctuate=true';
      
      console.log(`Deepgram: Connecting (attempt ${reconnectAttempts + 1})...`);
      
      dgConnection = new WebSocket(url, {
        headers: {
          Authorization: `Token ${apiKey}`
        }
      });

      dgConnection.on('open', () => {
        console.log('Deepgram WebSocket Opened (Linear16)');
        reconnectAttempts = 0; // reset on successful open
        
        // Flush any buffered audio
        if (pendingBuffer.length > 0) {
          console.log(`Deepgram: Flushing ${pendingBuffer.length} buffered audio chunks`);
          for (const buf of pendingBuffer) {
            dgConnection.send(buf);
          }
          pendingBuffer = [];
        }

        // Keep-alive every 5s
        if (keepAliveInterval) clearInterval(keepAliveInterval);
        keepAliveInterval = setInterval(() => {
          if (dgConnection && dgConnection.readyState === WebSocket.OPEN) {
            dgConnection.send(JSON.stringify({ type: 'KeepAlive' }));
          }
        }, 5000);

        resolve();
      });

      dgConnection.on('message', (data) => {
        try {
          const raw = data.toString();
          const res = JSON.parse(raw);
          
          if (res.type === 'Error' || res.error) {
            console.error('Deepgram API Error:', JSON.stringify(res));
            return;
          }
          
          if (res.type === 'Metadata') {
            console.log('Deepgram Metadata received (connection confirmed)');
            hasEverReceivedData = true;
            return;
          }

          if (res.channel && res.channel.alternatives) {
            hasEverReceivedData = true;
            const transcript = res.channel.alternatives[0].transcript;
            if (transcript && transcript.trim()) {
              onTranscript({
                text: transcript,
                isFinal: res.is_final,
                speechFinal: res.speech_final,
                isQuestion: transcript.includes('?')
              });
            }
          }
        } catch (e) {
          console.error('Deepgram parse error:', e.message);
        }
      });

      dgConnection.on('close', (code, reason) => {
        const reasonStr = reason ? reason.toString() : 'none';
        console.log(`Deepgram WebSocket Closed: Code ${code}, Reason: ${reasonStr}`);
        dgConnection = null;
        if (keepAliveInterval) {
          clearInterval(keepAliveInterval);
          keepAliveInterval = null;
        }

        if (isActive) {
          reconnectAttempts++;
          if (reconnectAttempts > MAX_RECONNECTS) {
            console.error(`Deepgram: Max reconnect attempts (${MAX_RECONNECTS}) reached. Stopping.`);
            isActive = false;
            pendingBuffer = [];
            if (onError) onError('Deepgram connection failed after multiple attempts. Check API key or network.');
            return;
          }
          // Exponential backoff: 1s, 2s, 4s, 8s, 16s
          const delay = Math.min(1000 * Math.pow(2, reconnectAttempts - 1), 16000);
          console.log(`Deepgram: Reconnecting in ${delay}ms (attempt ${reconnectAttempts}/${MAX_RECONNECTS})...`);
          if (reconnectTimer) clearTimeout(reconnectTimer);
          reconnectTimer = setTimeout(() => {
            if (isActive) connect();
          }, delay);
        }
      });

      dgConnection.on('error', (err) => {
        console.error('Deepgram WebSocket Error:', err.message);
        resolve();
      });
    });
  }

  function sendAudio(buffer) {
    if (dgConnection && dgConnection.readyState === WebSocket.OPEN) {
      dgConnection.send(buffer);
    } else if (dgConnection && dgConnection.readyState === WebSocket.CONNECTING) {
      pendingBuffer.push(buffer);
      if (pendingBuffer.length > 200) pendingBuffer.shift();
    } else if (isActive) {
      pendingBuffer.push(buffer);
      if (pendingBuffer.length > 200) pendingBuffer.shift();
      // Only try reconnect from sendAudio if we haven't exceeded retries
      if (reconnectAttempts < MAX_RECONNECTS) {
        connect();
      }
    }
  }

  function disconnect() {
    isActive = false;
    pendingBuffer = [];
    reconnectAttempts = 0;
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    if (keepAliveInterval) {
      clearInterval(keepAliveInterval);
      keepAliveInterval = null;
    }
    if (dgConnection) {
      dgConnection.close();
      dgConnection = null;
    }
  }

  return { connect, sendAudio, disconnect };
}

module.exports = { createDeepgramService };
