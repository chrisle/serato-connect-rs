// Generate synthetic fixtures and execute actual checksum-pinned TypeScript.
// No user directories, installed Serato files or network advertisements.
require('./verify-reference.cjs');
const fs=require('node:fs'),path=require('node:path'),crypto=require('node:crypto');
const esbuild=require('esbuild');
const root=path.resolve(__dirname,'..'),dir=path.join(root,'fixtures/metadata');fs.mkdirSync(dir,{recursive:true});
esbuild.buildSync({entryPoints:[path.join(__dirname,'reference/index.ts')],bundle:true,platform:'node',format:'cjs',outfile:path.join(__dirname,'reference.cjs'),external:['music-metadata','better-sqlite3-multiple-ciphers','bonjour-service']});
const ts=require('./reference.cjs');
const b32=n=>{const b=Buffer.alloc(4);b.writeUInt32BE(n);return b},le32=n=>{const b=Buffer.alloc(4);b.writeUInt32LE(n);return b};
const f32=n=>{const b=Buffer.alloc(4);b.writeFloatBE(n);return b},f64=n=>{const b=Buffer.alloc(8);b.writeDoubleBE(n);return b};
const text=s=>Buffer.from(s,'utf16le').swap16();
const chunk=(tag,data)=>Buffer.concat([Buffer.from(tag,'ascii'),b32(data.length),data]);
const atom=(tag,data)=>Buffer.concat([b32(data.length+8),Buffer.from(tag,'ascii'),data]);
const sync=n=>Buffer.from([(n>>>21)&127,(n>>>14)&127,(n>>>7)&127,n&127]);
const base=data=>data.toString('base64').replace(/=+$/,'');
const normalize=value=>JSON.parse(JSON.stringify(value,(_key,value)=>value instanceof Uint8Array?Array.from(value):value));
const cases=[];
async function record(op,args,run){let expected;try{expected={ok:normalize(await run())}}catch{expected={error:true}}cases.push({op,...args,expected});}
const hex=buf=>buf.toString('hex');
(async()=>{
for(const values of [[0,0,0],[255,255,255],[204,136,0],[1,127,128]]){
 await record('encodeSerato32',{values},()=>ts.encodeSerato32(...values));
 const encoded=Buffer.from(ts.encodeSerato32(...values));
 await record('decodeSerato32',{values:Array.from(encoded)},()=>ts.decodeSerato32(...encoded));
 const input=Buffer.from(values);
 await record('encodeSerato32Buffer',{hex:hex(input)},()=>hex(ts.encodeSerato32Buffer(input)));
 await record('decodeSerato32Buffer',{hex:hex(encoded)},()=>hex(ts.decodeSerato32Buffer(encoded)));
 await record('decodeSerato32U32',{hex:hex(Buffer.concat([Buffer.from([99]),encoded])),offset:1},()=>ts.decodeSerato32U32(Buffer.concat([Buffer.from([99]),encoded]),1));
 await record('decodeSerato32Color',{hex:hex(encoded),offset:0},()=>ts.decodeSerato32Color(encoded));
 await record('encodeSerato32Color',{color:{r:values[0],g:values[1],b:values[2]}},()=>hex(ts.encodeSerato32Color({r:values[0],g:values[1],b:values[2]})));
}
for(const value of [0,1,0xffffff,0x12345678])await record('encodeSerato32U32',{value},()=>hex(ts.encodeSerato32U32(value)));
for(const op of ['encodeSerato32Buffer','decodeSerato32Buffer'])await record(op,{hex:'00'},()=>hex(ts[op](Buffer.from([0]))));
for(const input of ['', 'AA','AA==','A','!!!!','SGVsbG8\nV29ybGQ',' SGVsbG8\uFEFFV29ybGQ ','-_8','a===b','abcde']){
 await record('decodeWithLinebreaks',{text:input},()=>hex(ts.decodeWithLinebreaks(input)));
 await record('isSeratoBase64',{text:input},()=>ts.isSeratoBase64(input));
}
for(const size of [0,1,54,55,120]){const input=Buffer.from(Array.from({length:size},(_,i)=>i&255));await record('encodeWithLinebreaks',{hex:hex(input)},()=>ts.encodeWithLinebreaks(input));}
const cue=(index,name)=>Buffer.concat([Buffer.from([0,index]),b32(12345+index),Buffer.from([0,12,34,56,0,0]),Buffer.from(name+'\0')]);
const loop=Buffer.concat([Buffer.from([0,2]),b32(4000),b32(8000),Buffer.alloc(4),Buffer.from([255,1,2,3,0,0,0,1]),Buffer.from('Loop\0')]);
const jump=Buffer.concat([Buffer.from([0]),b32(16),f64(1.25),f64(2.5)]),censor=Buffer.concat([Buffer.from([1]),b32(24),f64(3.25),f64(4.5),f64(-1)]);
const flip=Buffer.concat([Buffer.from([0,1,1]),Buffer.from('Flip\0'),Buffer.from([1]),b32(2),jump,censor]);
const marker=(name,data)=>Buffer.concat([Buffer.from(name+'\0'),b32(data.length),data]);
const markers=Buffer.concat([Buffer.from([1,1]),marker('COLOR',Buffer.from([0,204,136,0])),marker('BPMLOCK',Buffer.from([1])),marker('CUE',cue(5,'É cue')),marker('CUE',cue(0,'First')),marker('LOOP',loop),marker('FLIP',flip),marker('UNKNOWN',Buffer.from([1,2,3])),Buffer.from([0])]);
for(const data of [Buffer.alloc(0),Buffer.from([1,1,0]),markers,Buffer.concat([markers.subarray(0,-1),Buffer.from('BROKEN\0'),b32(999)])])await record('parseMarkers2',{hex:hex(data)},()=>ts.parseMarkers2(data));
await record('parseMarkers2',{hex:hex(Buffer.concat([Buffer.from([1,1]),marker('CUE',Buffer.from([0,1]))]))},()=>ts.parseMarkers2(Buffer.concat([Buffer.from([1,1]),marker('CUE',Buffer.from([0,1]))])));
for(const encoded of [base(markers),base(markers).match(/.{1,72}/g).join('\n'),base(markers)+'A'])await record('parseMarkers2FromBase64',{text:encoded},()=>ts.parseMarkers2FromBase64(encoded));
const geobMarkers=Buffer.concat([Buffer.from([1,1]),Buffer.from(base(markers)),Buffer.alloc(470)]);
await record('parseMarkers2FromGeob',{hex:hex(geobMarkers)},()=>ts.parseMarkers2FromGeob(geobMarkers));
const grid=Buffer.concat([Buffer.from([1,1]),b32(2),f32(1.5),b32(8),f32(5.5),f32(128.25),Buffer.from([0])]);
for(const data of [Buffer.alloc(0),grid,grid.subarray(0,10)])await record('parseBeatgrid',{hex:hex(data)},()=>ts.parseBeatgrid(data));
await record('parseBeatgridFromBase64',{text:base(grid)},()=>ts.parseBeatgridFromBase64(base(grid)));
for(const value of [ts.parseBeatgrid(grid),{markers:[]},{markers:[{position:0,bpm:99}]}]){await record('getBpmFromBeatgrid',{grid:value},()=>ts.getBpmFromBeatgrid(value)??null);await record('isDynamicBeatgrid',{grid:value},()=>ts.isDynamicBeatgrid(value));}
const autotags=Buffer.concat([Buffer.from([1,1]),Buffer.from('128.50\0-3.257\0'+'0.000\0','ascii')]);
for(const data of [Buffer.alloc(0),autotags,Buffer.from('12.5xx\0badxxx\0-2.00\0')])await record('parseAutotags',{hex:hex(data)},()=>ts.parseAutotags(data));
await record('parseAutotagsFromBase64',{text:base(autotags)},()=>ts.parseAutotagsFromBase64(base(autotags)));
const crate=Buffer.concat([chunk('vrsn',text('1.0/Serato ScratchLive Crate')),chunk('otrk',Buffer.concat([chunk('ptrk',text('/Synthetic/É one.mp3')),chunk('ptrk',text('Music/Album/two.flac')),chunk('ptrk',text(''))]))]);
fs.mkdirSync(path.join(dir,'Subcrates'),{recursive:true});fs.writeFileSync(path.join(dir,'Subcrates/A.crate'),crate);fs.writeFileSync(path.join(dir,'Subcrates/B.crate'),chunk('otrk',chunk('ptrk',text('Music/Album/two.flac'))));fs.writeFileSync(path.join(dir,'Subcrates/ignored.scrate'),crate);
for(const method of ['parseCrate','parseCrateSync'])await record(method,{file:'Subcrates/A.crate'},async()=>{const v=await ts[method](path.join(dir,'Subcrates/A.crate'));v.path='Subcrates/A.crate';return v});
await record('listCrates',{},async()=>(await ts.listCrates(dir)).map(p=>path.relative(dir,p).split(path.sep).join('/')));
await record('getAllCrates',{},async()=>(await ts.getAllCrates(dir)).map(v=>({...v,path:path.relative(dir,v.path).split(path.sep).join('/')})));
for(const track of ['/Synthetic/é ONE.mp3','/Drive/Music/Album/two.flac','not-found'])await record('findCratesForTrack',{track},()=>ts.findCratesForTrack(dir,track));
const field=(tag,value)=>chunk(tag,tag[0]==='b'?Buffer.from([Number(value)]):tag[0]==='u'?b32(value):text(value));
const database=Buffer.concat([chunk('vrsn',text('2.0')),chunk('otrk',Buffer.concat([
 field('pfil','/Synthetic/É one.mp3'),field('tsng','One'),field('tart','Artist'),field('talb','Album'),field('tgen','House'),field('tkey','8A'),field('tbpm','128.5junk'),field('tlen','300.25'),field('tbit','320kbps'),field('tsmp','44100Hz'),field('ttyp','mp3'),field('bbgl',true),field('bmis',false),field('uadd',1700000000),field('tcom','Search me'),field('tgrp','Group'),field('tcmp','Composer'),field('tlbl','Label'),field('ttyr','2026suffix')
 ])),chunk('otrk',Buffer.concat([field('pfil','Music/Album/two.flac'),field('tsng','Two'),field('tart','Artist'),field('tgen','Techno'),field('tbpm','0'),field('bmis',true)])),chunk('otrk',field('tsng','Missing path'))]);
fs.writeFileSync(path.join(dir,'database V2'),database);
for(const method of ['parseDatabase','parseDatabaseSync'])await record(method,{file:'database V2'},()=>ts[method](path.join(dir,'database V2')));
await record('getLibraryTracks',{},()=>ts.getLibraryTracks(dir));
for(const track of ['/synthetic/é ONE.mp3','two.flac','missing','/Synthetic/É one.mp3/','../../two.flac'])await record('getTrackByPath',{track},async()=>await ts.getTrackByPath(dir,track)??null);
for(const query of ['HOUSE','search ME','artist','', 'missing'])await record('searchLibrary',{query},()=>ts.searchLibrary(dir,query));
await record('getDatabaseStats',{},()=>ts.getDatabaseStats(dir));
const unicodeDir=path.join(dir,'unicode');fs.mkdirSync(path.join(unicodeDir,'Subcrates'),{recursive:true});
for(const name of ['😀','\uE000'])fs.writeFileSync(path.join(unicodeDir,'Subcrates',name+'.crate'),Buffer.alloc(0));
await record('listCrates',{folder:'unicode'},async()=>(await ts.listCrates(unicodeDir)).map(p=>path.relative(dir,p).split(path.sep).join('/')));
fs.writeFileSync(path.join(unicodeDir,'database V2'),Buffer.concat(['😀','\uE000'].map((genre,index)=>chunk('otrk',Buffer.concat([field('pfil','/Synthetic/'+index),field('tgen',genre)])))));
await record('getDatabaseStats',{folder:'unicode'},()=>ts.getDatabaseStats(unicodeDir));
const payloads=[['Serato Markers2',geobMarkers],['Serato BeatGrid',grid],['Serato Autotags',autotags],['Serato Overview',Buffer.from([1,2,255,0])]];
function makeId3(version=4,selected=payloads){const frames=selected.map(([name,data])=>{const body=Buffer.concat([Buffer.from([0]),Buffer.from('application/octet-stream\0\0'+name+'\0'),data]);return Buffer.concat([Buffer.from('GEOB'),version===4?sync(body.length):b32(body.length),Buffer.alloc(2),body]);});const data=Buffer.concat(frames);return Buffer.concat([Buffer.from([73,68,51,version,0,0]),sync(data.length),data]);}
function makeId3v22(){const frames=payloads.map(([name,data])=>{const body=Buffer.concat([Buffer.from([0]),Buffer.from('application/octet-stream\0\0'+name+'\0'),data]);const length=Buffer.from([(body.length>>>16)&255,(body.length>>>8)&255,body.length&255]);return Buffer.concat([Buffer.from('GEO'),length,body]);});const data=Buffer.concat(frames);return Buffer.concat([Buffer.from([73,68,51,2,0,0]),sync(data.length),data]);}
function makeId3Utf16(){const frames=payloads.map(([name,data])=>{const utf16=s=>Buffer.concat([Buffer.from([255,254]),Buffer.from(s+'\0','utf16le')]);const body=Buffer.concat([Buffer.from([1]),Buffer.from('application/octet-stream\0'),utf16(''),utf16(name),data]);return Buffer.concat([Buffer.from('GEOB'),sync(body.length),Buffer.alloc(2),body]);});const data=Buffer.concat(frames);return Buffer.concat([Buffer.from([73,68,51,4,0,0]),sync(data.length),data]);}
const mp3Audio=Buffer.concat(Array.from({length:3},()=>Buffer.concat([Buffer.from([0xff,0xfb,0x90,0]),Buffer.alloc(413)])));
const comments=Buffer.concat([le32(9),Buffer.from('Synthetic'),le32(4),...['SERATO_MARKERS_V2','SERATO_BEATGRID','SERATO_AUTOTAGS','SERATO_OVERVIEW'].map((name,i)=>{const value=Buffer.from(name+'='+base([markers,grid,autotags,Buffer.from([1,2,255,0])][i]));return Buffer.concat([le32(value.length),value]);})]);
const files=[];function audio(name,mime,data){fs.writeFileSync(path.join(dir,name),data);files.push({name,mime});}
audio('synthetic-v24.mp3','audio/mpeg',Buffer.concat([makeId3(4),mp3Audio]));audio('synthetic-v23.mp3','audio/mpeg',Buffer.concat([makeId3(3),mp3Audio]));
audio('synthetic-v22.mp3','audio/mpeg',Buffer.concat([makeId3v22(),mp3Audio]));
audio('synthetic-utf16.mp3','audio/mpeg',Buffer.concat([makeId3Utf16(),mp3Audio]));
const badMarkers=Buffer.concat([Buffer.from([1,1]),marker('CUE',Buffer.from([0,1]))]);
const badGeob=Buffer.concat([Buffer.from([1,1]),Buffer.from(base(badMarkers))]);
audio('synthetic-bad-marker.mp3','audio/mpeg',Buffer.concat([makeId3(4,[['Serato Markers2',badGeob],...payloads.slice(1)]),mp3Audio]));
const comm=Buffer.alloc(18);comm.writeUInt16BE(2);comm.writeUInt32BE(1,2);comm.writeUInt16BE(16,6);Buffer.from('400eac44000000000000','hex').copy(comm,8);
const aiffChunks=Buffer.concat([chunk('COMM',comm),chunk('ID3 ',makeId3()),chunk('SSND',Buffer.alloc(12))]);audio('synthetic.aiff','audio/aiff',Buffer.concat([Buffer.from('FORM'),b32(aiffChunks.length+4),Buffer.from('AIFF'),aiffChunks]));
const riffChunk=(tag,data)=>Buffer.concat([Buffer.from(tag),le32(data.length),data,...(data.length%2?[Buffer.from([0])]:[])]);
const fmt=Buffer.alloc(16);fmt.writeUInt16LE(1);fmt.writeUInt16LE(2,2);fmt.writeUInt32LE(44100,4);fmt.writeUInt32LE(176400,8);fmt.writeUInt16LE(4,12);fmt.writeUInt16LE(16,14);
const waveChunks=Buffer.concat([riffChunk('fmt ',fmt),riffChunk('id3 ',makeId3()),riffChunk('data',Buffer.alloc(4))]);
audio('synthetic.wav','audio/wav',Buffer.concat([Buffer.from('RIFF'),le32(waveChunks.length+4),Buffer.from('WAVE'),waveChunks]));
const stream=Buffer.alloc(34);stream.writeUInt16BE(4096);stream.writeUInt16BE(4096,2);stream.writeBigUInt64BE((44100n<<44n)|(1n<<41n)|(15n<<36n)|1n,10);
audio('synthetic.flac','audio/flac',Buffer.concat([Buffer.from('fLaC'),Buffer.from([0,0,0,34]),stream,Buffer.from([0x84,(comments.length>>>16)&255,(comments.length>>>8)&255,comments.length&255]),comments]));
const emptyComments=Buffer.concat([le32(0),le32(4),...['SERATO_MARKERS_V2','SERATO_BEATGRID','SERATO_AUTOTAGS','SERATO_OVERVIEW'].map(name=>{const value=Buffer.from(name+'=');return Buffer.concat([le32(value.length),value]);})]);
audio('synthetic-empty.flac','audio/flac',Buffer.concat([Buffer.from('fLaC'),Buffer.from([0,0,0,34]),stream,Buffer.from([0x84,(emptyComments.length>>>16)&255,(emptyComments.length>>>8)&255,emptyComments.length&255]),emptyComments]));
function page(packet,seq,flag){const laces=[];for(let n=packet.length;n>=255;n-=255)laces.push(255);laces.push(packet.length%255);const h=Buffer.alloc(27);h.write('OggS');h[5]=flag;h.writeUInt32LE(42,14);h.writeUInt32LE(seq,18);h[26]=laces.length;return Buffer.concat([h,Buffer.from(laces),packet]);}
const ident=Buffer.alloc(30);ident[0]=1;ident.write('vorbis',1);ident[11]=2;ident.writeUInt32LE(44100,12);ident[28]=0xb8;ident[29]=1;
audio('synthetic.ogg','audio/ogg',Buffer.concat([page(ident,0,2),page(Buffer.concat([Buffer.from('\x03vorbis'),comments,Buffer.from([1])]),1,0),page(Buffer.from([0]),2,4)]));
const commentPacket=Buffer.concat([Buffer.from('\x03vorbis'),comments,Buffer.from([1])]);
let firstCommentPage=page(commentPacket.subarray(0,255),1,0);
firstCommentPage=Buffer.concat([firstCommentPage.subarray(0,28),firstCommentPage.subarray(29)]);firstCommentPage[26]=1;
audio('synthetic-fragmented.ogg','audio/ogg',Buffer.concat([page(ident,0,2),firstCommentPage,page(commentPacket.subarray(255),2,1),page(Buffer.from([0]),3,4)]));
const custom=(name,data)=>atom('----',Buffer.concat([atom('mean',Buffer.concat([Buffer.alloc(4),Buffer.from('com.serato.dj')])),atom('name',Buffer.concat([Buffer.alloc(4),Buffer.from(name)])),atom('data',Buffer.concat([Buffer.alloc(8),data]))]));
const ilst=atom('ilst',Buffer.concat(['markersv2','beatgrid','autotags','overview'].map((name,i)=>custom(name,[markers,grid,autotags,Buffer.from([1,2,255,0])][i]))));
audio('synthetic.m4a','audio/mp4',Buffer.concat([atom('ftyp',Buffer.concat([Buffer.from('M4A '),Buffer.alloc(4),Buffer.from('isomM4A ')])),atom('moov',atom('udta',atom('meta',Buffer.concat([Buffer.alloc(4),ilst]))))]));
for(const {name,mime}of files){const file=path.join(dir,name),data=fs.readFileSync(file);await record('getTrackMetadataFromBuffer',{file:name,mime},()=>ts.getTrackMetadataFromBuffer(data,mime));await record('getTrackMetadata',{file:name,mime},()=>ts.getTrackMetadata(file));}
for(const [name,mime]of [['synthetic.flac','audio/mpeg'],['synthetic-v24.mp3','audio/flac'],['synthetic.flac','application/octet-stream'],['synthetic.flac',''],['synthetic.ogg','audio/flac']]){
 await record('getTrackMetadataFromBuffer',{file:name,mime,mismatchedHint:true},()=>ts.getTrackMetadataFromBuffer(fs.readFileSync(path.join(dir,name)),mime));
}
fs.writeFileSync(path.join(dir,'invalid.audio'),Buffer.from('not audio'));
await record('getTrackMetadataFromBuffer',{file:'invalid.audio',mime:'audio/mpeg'},()=>ts.getTrackMetadataFromBuffer(Buffer.from('not audio'),'audio/mpeg'));
const expected=path.join(root,'fixtures/metadata-expected.json');fs.writeFileSync(expected,JSON.stringify(cases,null,2)+'\n');
const hashes={};for(const item of fs.readdirSync(dir,{recursive:true}).sort()){const p=path.join(dir,item);if(fs.statSync(p).isFile())hashes[item.split(path.sep).join('/')]=crypto.createHash('sha256').update(fs.readFileSync(p)).digest('hex');}
fs.writeFileSync(path.join(root,'fixtures/metadata-hashes.json'),JSON.stringify(hashes,null,2)+'\n');
const audioCases=cases.filter(c=>c.op.startsWith('getTrackMetadata'));
console.log(`Actual TS oracle produced ${cases.length} cases; audio: ${audioCases.map(c=>c.file+':'+(c.expected.ok?'metadata':'null/error')).join(', ')}`);
for(const c of audioCases.filter(c=>['synthetic.m4a','synthetic-v22.mp3'].includes(c.file))){
 if(c.expected.ok!==null)throw Error('Review MP4 upstream behavior changed');
 c.correction=c.file==='synthetic.m4a'?'raw-mp4-serato-atoms':'id3v22-geob-alias';
 c.expectedNative={ok:normalize({markers:ts.parseMarkers2(markers),beatgrid:ts.parseBeatgrid(grid),autotags:ts.parseAutotags(autotags),overview:Uint8Array.from([1,2,255,0])})};
}
fs.writeFileSync(expected,JSON.stringify(cases,null,2)+'\n');
if(audioCases.some(c=>!c.mismatchedHint && !['invalid.audio','synthetic.m4a','synthetic-v22.mp3','synthetic-empty.flac'].includes(c.file) && !c.expected.ok))throw Error('Synthetic valid audio fixture did not reach original reader');
})().catch(error=>{console.error(error);process.exitCode=1;});
