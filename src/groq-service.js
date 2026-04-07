const Groq = require('groq-sdk');

const TEXT_MODEL = 'llama-3.3-70b-versatile';
const VISION_MODEL = 'meta-llama/llama-4-scout-17b-16e-instruct';

function createGroqService(apiKey) {
  const client = new Groq({ apiKey });
  let conversationHistory = [];

  function addToHistory(role, content) {
    conversationHistory.push({ role, content });
    if (conversationHistory.length > 40) conversationHistory = conversationHistory.slice(-30);
  }

  function clearHistory() {
    conversationHistory = [];
  }

  function buildTranscriptText(transcripts) {
    if (!transcripts || transcripts.length === 0) return '';
    return transcripts.map(t => `[${t.source || 'unknown'}]: ${t.text}`).join('\n');
  }

  function buildContextText(messages) {
    if (!messages || messages.length === 0) return '';
    return messages.map(m => `[${m.role}]: ${m.content}`).join('\n');
  }

  async function callGroq(systemPrompt, userContent, images = []) {
    try {
      const model = images.length > 0 ? VISION_MODEL : TEXT_MODEL;
      const messages = [{ role: 'system', content: systemPrompt }];

      // Add conversation history for context
      conversationHistory.forEach(h => messages.push(h));

      // Build user message
      if (images.length > 0) {
        const content = [{ type: 'text', text: userContent }];
        images.forEach(b64 => {
          content.push({
            type: 'image_url',
            image_url: { url: `data:image/png;base64,${b64}` }
          });
        });
        messages.push({ role: 'user', content });
      } else {
        messages.push({ role: 'user', content: userContent });
      }

      const response = await client.chat.completions.create({
        model,
        messages,
        max_tokens: 4096,
        temperature: 0.3
      });

      const reply = response.choices[0]?.message?.content || 'No response generated.';
      addToHistory('user', typeof userContent === 'string' ? userContent.substring(0, 200) : 'User query');
      addToHistory('assistant', reply.substring(0, 500));
      return { success: true, response: reply };
    } catch (err) {
      console.error('Groq API error:', err.message);
      return { success: false, error: err.message };
    }
  }

  async function askAI(transcripts, contextMessages, images) {
    const transcript = buildTranscriptText(transcripts);
    const context = buildContextText(contextMessages);
    const system = `You are GhostAI, an expert AI assistant for technical interviews, coding sessions, and meetings.
Read all transcript messages as a single conversation. Fix speech-to-text errors silently.
Identify the real question being asked across all fragments.

RESPOND WITH:
**Understanding:** [1 sentence — what you understood]
**Answer:** [Complete response]

For coding/algorithm questions:
**Approach:** [Key idea]
**Solution:**
\`\`\`python
[Complete runnable code]
\`\`\`
**Complexity:** Time: O(?) | Space: O(?)
**Key Points:** [Important takeaways]

Rules:
- Synthesize ALL transcript messages
- Screenshots override transcript when they conflict
- Give complete answers, not partial`;

    const userMsg = [
      transcript ? `TRANSCRIPT:\n${transcript}` : '',
      context ? `CONTEXT:\n${context}` : '',
      images.length ? `[${images.length} screenshot(s) attached]` : '',
      'Please analyze and respond.'
    ].filter(Boolean).join('\n\n');

    return await callGroq(system, userMsg, images);
  }

  async function screenAI(contextMessages, images) {
    const context = buildContextText(contextMessages);
    const system = `You are GhostAI, an expert programming assistant. Analyze the screenshot(s).
Read ALL visible text — constraints, code, errors, function signatures, sample I/O.
Identify the content type and respond accordingly.

For coding problems:
**Understanding:** [What the screenshot shows]
**Approach:** [Key algorithm/fix]
**Solution:**
\`\`\`python
[Complete runnable code]
\`\`\`
**Complexity:** Time: O(?) | Space: O(?)

For non-coding screenshots:
**What I see:** [Description]
**Answer:** [Direct response]
**Key Points:** [Important observations]`;

    const userMsg = [
      context ? `CONTEXT:\n${context}` : '',
      `[${images.length} screenshot(s) to analyze]`,
      'Analyze these screenshots and respond.'
    ].filter(Boolean).join('\n\n');

    return await callGroq(system, userMsg, images);
  }

  async function suggest(transcripts, contextMessages) {
    const transcript = buildTranscriptText(transcripts);
    const context = buildContextText(contextMessages);
    const system = `You are GhostAI, a real-time conversation coach. Read the transcript and suggest what to say next.
Fix speech-to-text errors silently.

RESPOND WITH:
**Best response (say this):** [2-4 sentences, natural spoken language, ready to say out loud]
**Key points:** [2-3 short anchor concepts]
**Follow-ups:** [What they'll likely ask next]

Rules:
- Must be speakable natural language
- Hit the headline, not every detail
- Don't echo the transcript`;

    const userMsg = [
      transcript ? `TRANSCRIPT:\n${transcript}` : '',
      context ? `CONTEXT:\n${context}` : '',
      'What should I say next?'
    ].filter(Boolean).join('\n\n');

    return await callGroq(system, userMsg);
  }

  async function notes(transcripts, contextMessages) {
    const transcript = buildTranscriptText(transcripts);
    const context = buildContextText(contextMessages);
    const system = `You are GhostAI. Generate structured notes from the conversation. Fix STT errors silently.

OUTPUT:
## Key Discussion Points
- [Main topics]

## Decisions Made
- [Decisions with owners]

## Action Items
- [ ] [Task] — Owner: [name] | Deadline: [if mentioned]

## Open Questions
- [Unresolved items]

## Next Steps
- [What happens next]

If a section has nothing, write "None noted."`;

    const userMsg = [
      transcript ? `TRANSCRIPT:\n${transcript}` : '',
      context ? `CONTEXT:\n${context}` : '',
      'Generate structured notes.'
    ].filter(Boolean).join('\n\n');
    return await callGroq(system, userMsg);
  }

  function transcribe(filePath) {
    return client.audio.transcriptions.create({
      file: require('fs').createReadStream(filePath),
      model: 'whisper-large-v3',
      response_format: 'json',
      language: 'en'
    }).then(res => ({ success: true, text: res.text }))
      .catch(err => ({ success: false, error: err.message }));
  }

  return { askAI, screenAI, suggest, notes, transcribe, clearHistory, addToHistory };
}

module.exports = { createGroqService };
