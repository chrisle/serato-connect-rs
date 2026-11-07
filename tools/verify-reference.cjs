const fs=require('node:fs'),path=require('node:path'),crypto=require('node:crypto');
const provenance=require('../fixtures/PROVENANCE.json');
for(const [name,hash]of Object.entries(provenance.source_sha256)){
 const actual=crypto.createHash('sha256').update(fs.readFileSync(path.join(__dirname,'reference',name))).digest('hex');
 if(actual!==hash)throw Error(`Pinned original source checksum mismatch: ${name}`);
}
console.log('Pinned Serato TypeScript source hashes verified');
