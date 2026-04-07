const { Deepgram } = require("@deepgram/sdk");
console.log("Deepgram:", typeof Deepgram);
const client = new Deepgram("fake");
console.log("Client properties:", Object.keys(client));
