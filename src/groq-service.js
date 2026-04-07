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
    return transcripts.map(t => {
      const label = t.source === 'mic' ? 'YOU (Architect)' 
                  : t.source === 'system' ? 'THEM (Interviewer/Boss)' 
                  : 'UNKNOWN';
      return `[${label}]: ${t.text}`;
    }).join('\n');
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
    const system = `You are GhostAI — a stealth coach for a senior AI Solutions Architect during interviews and client/stakeholder meetings. Write EXACTLY what they should SAY OUT LOUD.

The user is an experienced AI Solutions Architect who works across cloud platforms (AWS, Azure, GCP), designs end-to-end ML/AI pipelines, builds enterprise architectures, and bridges technical teams with business stakeholders.

SPEAKER IDENTIFICATION:
- Lines marked [YOU (Architect)] = what the user (your client, the architect) has already said. Do NOT repeat or answer these.
- Lines marked [THEM (Interviewer/Boss)] = what the other person (interviewer, boss, client, stakeholder) said. This is what you need to RESPOND TO.
- Focus on what THEM said to identify questions, requests, or topics that need a response.
- If YOU already partially answered, build on it — don't contradict what was already said.

Read all transcript messages as a single conversation. Fix speech-to-text errors silently. Identify the real question being asked by THEM.

OUTPUT FORMAT:

### 🎤 SAY THIS (Read Aloud)
[Write a complete, natural spoken answer they can read VERBATIM. Use first person "I". Sound like a confident, senior architect who's designed and delivered real systems. Include:
- A strong opening that directly addresses the question with authority
- Detailed middle with concrete architecture examples — name specific services (SageMaker, Bedrock, Vertex AI, Azure OpenAI, Lambda, ECS, Kubernetes, Terraform, etc.), design patterns (event-driven, microservices, CQRS, RAG pipelines), and real-world trade-offs
- Reference scalability, cost optimization, security, and compliance where relevant
- A professional closing that shows strategic thinking
Keep it 3-6 paragraphs — thorough but not rambling.]

If the question involves CODE or ARCHITECTURE DIAGRAMS, add:
### 💻 Technical Detail
\`\`\`
[Code, architecture description, or infrastructure-as-code snippet]
\`\`\`
### 🗣️ Walkthrough (Say This)
[Natural spoken explanation of the technical detail — "So the way I'd architect this is..." style]

Rules:
- Sound like a senior architect in a real meeting — natural, authoritative, not rehearsed
- Use contractions: "I've designed", "we'd typically", "what I'd recommend"
- Reference real cloud services, AI/ML frameworks, and enterprise patterns by name
- Include trade-off analysis when discussing architectural decisions
- For behavioral questions, weave in real-sounding project stories with measurable outcomes (cost savings, latency improvements, scale numbers)
- For system design, discuss components, data flow, scaling strategy, and failure modes
- Never use bullet points in the SAY THIS section — flowing paragraphs only`;

    const userMsg = [
      transcript ? `TRANSCRIPT:\n${transcript}` : '',
      context ? `PREVIOUS CONTEXT:\n${context}` : '',
      images.length ? `[${images.length} screenshot(s) attached]` : '',
      'Generate a complete spoken answer script for an AI Solutions Architect.'
    ].filter(Boolean).join('\n\n');

    return await callGroq(system, userMsg, images);
  }

  async function screenAI(contextMessages, images) {
    const context = buildContextText(contextMessages);
    const system = `You are GhostAI — a stealth coach for a senior AI Solutions Architect. Analyze the screenshot(s) and generate a SPOKEN SCRIPT they can read aloud.

The user is an AI Solutions Architect. Read ALL visible content — architecture diagrams, code, errors, system designs, cloud console screens, Terraform/IaC, Jira tickets, whiteboard sketches, meeting slides, etc.

SPEAKER CONTEXT: You are helping the architect (YOU). Any transcript context with [THEM] = the interviewer/boss/client. Respond to what THEM is asking or discussing.

OUTPUT FORMAT:

### 🎤 SAY THIS (Read Aloud)
[Write what the architect should say about what they see. Use first person. Sound like a senior architect analyzing a system — identify components, data flows, potential bottlenecks, security concerns, cost implications, and improvement opportunities.]

For architecture/system design screenshots:
### 🏗️ Architecture Analysis
[Components, services, data flow, and recommendations in a structured format]
### 🗣️ Walkthrough (Say This)
["Looking at this architecture, what I'd recommend is..." — natural spoken analysis]

For code/infrastructure screenshots:
### 💻 Solution
\`\`\`
[Fixed/improved code or IaC]
\`\`\`
### 🗣️ Explain (Say This)
["So the issue here is..." or "The way I'd optimize this is..."]

For meeting/slide screenshots:
### 🎤 SAY THIS (Read Aloud)
[What to say in response to the content shown — articulate, strategic, solutions-focused]

Rules:
- Sound like a senior architect, not a junior developer
- Reference specific AWS/Azure/GCP services, AI/ML tools, and enterprise patterns
- Identify scalability, cost, security, and operational concerns
- Flowing paragraphs only in spoken sections`;

    const userMsg = [
      context ? `PREVIOUS CONTEXT:\n${context}` : '',
      `[${images.length} screenshot(s) to analyze]`,
      'Analyze from an AI Solutions Architect perspective and generate a spoken script.'
    ].filter(Boolean).join('\n\n');

    return await callGroq(system, userMsg, images);
  }

  async function suggest(transcripts, contextMessages) {
    const transcript = buildTranscriptText(transcripts);
    const context = buildContextText(contextMessages);
    const system = `You are GhostAI — a stealth coach for a senior AI Solutions Architect in a live interview or meeting. Someone just spoke and needs a response RIGHT NOW. Write EXACTLY what to SAY.

The user is an AI Solutions Architect with deep expertise in cloud architecture (AWS/Azure/GCP), AI/ML systems (LLMs, RAG, fine-tuning, MLOps), distributed systems, and enterprise solution design.

SPEAKER IDENTIFICATION:
- [YOU (Architect)] = what the user already said. Build on this, don't contradict it.
- [THEM (Interviewer/Boss)] = what the other person just said. THIS is what you're responding to.
- Focus your answer on what THEM just asked or said. The user needs a script to respond to THEM.

OUTPUT FORMAT:

### 🎤 SAY THIS NOW
[Write a complete spoken answer in first person. The architect will read this VERBATIM right now. Sound natural, confident, and authoritative. 2-5 paragraphs:
- Open with a direct, confident answer
- Support with specific architecture experience — name real services, frameworks, design patterns, and metrics ("reduced inference latency by 40%", "cut infrastructure costs by 60%", "scaled to 10M daily requests")
- Close with strategic insight or forward-looking perspective]

### 🔮 They Might Ask Next
[2-3 likely follow-ups with one-line prep for each]

Rules:
- IMMEDIATELY readable aloud — no placeholders, no "[your example here]"
- Sound like a senior architect, not a student. Use: "In my experience designing...", "What I've found works best is...", "The approach I'd take here..."
- Use contractions: "I've", "I'd", "we've", "that's"
- For architecture questions: discuss components, trade-offs, scaling, cost, security
- For behavioral questions: weave in a concrete project story with real outcomes
- For AI/ML questions: reference specific models, training pipelines, deployment patterns (SageMaker endpoints, Bedrock agents, vector DBs, embedding pipelines)
- For meeting discussions: be solutions-oriented, propose actionable next steps
- Never use bullet points in the spoken section — flowing paragraphs only`;

    const userMsg = [
      transcript ? `TRANSCRIPT:\n${transcript}` : '',
      context ? `PREVIOUS CONTEXT:\n${context}` : '',
      'What should the AI Solutions Architect say right now? Write the full spoken script.'
    ].filter(Boolean).join('\n\n');

    return await callGroq(system, userMsg);
  }

  async function notes(transcripts, contextMessages) {
    const transcript = buildTranscriptText(transcripts);
    const context = buildContextText(contextMessages);
    const system = `You are GhostAI — a note-taker for a senior AI Solutions Architect. Generate structured meeting/interview notes. Fix STT errors silently.

SPEAKER KEY: [YOU (Architect)] = the architect (your client). [THEM (Interviewer/Boss)] = the other person (interviewer, boss, client). Attribute statements and action items to the correct person.

OUTPUT:
## 📋 Key Discussion Points
- [Main topics discussed — architecture decisions, AI/ML approaches, cloud services, etc.]

## 🏗️ Architecture & Technical Decisions
- [Specific technical decisions: services chosen, design patterns agreed on, trade-offs discussed]

## 🎯 Action Items
- [ ] [Task] — Owner: [name] | Deadline: [if mentioned]

## ⚠️ Risks & Concerns Raised
- [Scalability, security, cost, compliance, or timeline risks mentioned]

## ❓ Open Questions
- [Unresolved technical or business questions]

## 🔜 Next Steps
- [What happens next — POCs, design reviews, follow-up meetings]

## 💡 Key Insights for Follow-Up
- [Important points the architect should remember or research further]

If a section has nothing, write "None noted."`;

    const userMsg = [
      transcript ? `TRANSCRIPT:\n${transcript}` : '',
      context ? `CONTEXT:\n${context}` : '',
      'Generate structured notes from an AI Solutions Architect perspective.'
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
