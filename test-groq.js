const { createGroqService } = require('./src/groq-service');
const state = require('./cache/app-state.json');

async function test() {
  console.log('Testing Groq Key:', state.groqApiKey);
  try {
    const groq = createGroqService(state.groqApiKey);
    
    console.log('Testing Ask AI...');
    const result = await groq.askAI(
      [{ source: 'mic', text: 'What is 2 + 2?' }],
      [],
      []
    );
    
    if (result.success) {
      console.log('SUCCESS!');
      console.log(result.response);
    } else {
      console.error('FAILED RESULT:', result.error);
    }
  } catch (err) {
    console.error('CAUGHT ERROR:', err.message, err.stack);
  }
}

test();
