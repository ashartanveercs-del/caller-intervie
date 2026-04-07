const dg = require("@deepgram/sdk");
console.log("Deepgram Exports:", Object.keys(dg));
if (dg.createClient) console.log("createClient found");
else console.log("createClient NOT found");
