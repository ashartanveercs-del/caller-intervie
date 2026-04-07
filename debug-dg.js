const dg = require("@deepgram/sdk");
console.log("Full Exports Keys:", Object.keys(dg));
for (const key of Object.keys(dg)) {
  console.log(`Key: ${key}, Type: ${typeof dg[key]}`);
}
if (dg.default) {
  console.log("Default Keys:", Object.keys(dg.default));
}
