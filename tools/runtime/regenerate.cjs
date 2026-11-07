// Execute checksum-pinned TS using only generated binary/SQLite fixtures and
// pure OSC/client dispatch. No Serato discovery, native mDNS or personal IO.
require('../verify-reference.cjs');
const fs=require('fs'),os=require('os'),path=require('path');
const ts=require('../reference.cjs'),Database=require('../node_modules/better-sqlite3-multiple-ciphers');
const dir=fs.mkdtempSync(path.join(os.tmpdir(),'serato-runtime-oracle-'));
const target=__dirname;
const u32=n=>{const b=Buffer.alloc(4);b.writeUInt32BE(n);return b};
const chunk=(tag,data)=>Buffer.concat([Buffer.isBuffer(tag)?tag:Buffer.from(tag,'ascii'),u32(data.length),data]);
const text=s=>Buffer.from(s,'utf16le').swap16();
const field=(n,data)=>chunk(u32(n),data);
const record=({title,artist,filePath,bpm,start,play,played,deck})=>chunk('oent',chunk('adat',Buffer.concat([
field(6,text(title)),field(7,text(artist)),field(2,text(filePath)),field(15,u32(bpm)),field(28,u32(start)),
...(play===undefined?[]:[field(45,u32(play))]),field(50,Buffer.from([played])),field(31,u32(deck))])));
const songs=[{title:'Fixture 🎵',artist:'Synthetic Artist',filePath:'/Synthetic/one.mp3',bpm:128,start:1700000000,played:0,deck:1},
{title:'Played',artist:'Guest',filePath:'/Synthetic/two.mp3',bpm:129,start:1700000001,played:1,deck:2},
{title:'Ended',artist:'Guest',filePath:'/Synthetic/three.mp3',bpm:130,start:1700000002,play:1700000100,played:1,deck:2},
{title:'Deckless',artist:'',filePath:'',bpm:0,start:0,played:1,deck:0}];
const session=Buffer.concat(songs.map(record));
const sessions=Buffer.concat([chunk('oses',chunk('adat',Buffer.concat([field(1,u32(1)),field(41,text('2023-11-14'))]))),chunk('oses',chunk('adat',Buffer.concat([field(1,u32(2)),field(41,text('2023-11-15'))])))]);
function normalize(message){return {address:message.address,args:message.args.map(a=>({type:a.type,value:a.type==='b'?Array.from(a.value):a.value}))};}
(async()=>{try{
const historyRoot=path.join(dir,'v3');fs.mkdirSync(path.join(historyRoot,'History/Sessions'),{recursive:true});fs.writeFileSync(path.join(historyRoot,'History/history.database'),sessions);for(const id of [1,2])fs.writeFileSync(path.join(historyRoot,`History/Sessions/${id}.session`),session);
fs.writeFileSync(path.join(target,'session.bin'),session);fs.writeFileSync(path.join(target,'sessions.bin'),sessions);
const out={v3Songs:await ts.getSessionSongs(path.join(historyRoot,'History/Sessions/2.session')),v3Sessions:Array.from(await ts.getSessions(path.join(historyRoot,'History/history.database'))),v3History:await ts.getSeratoHistory(historyRoot),latestPath:path.basename(ts.getLatestSessionPath(historyRoot))};
const db=new Database(path.join(dir,'master.sqlite'));db.exec(fs.readFileSync(path.join(target,'schema.sql'),'utf8'));db.close();
out.v4Sessions=ts.getSeratoV4Sessions(dir);out.v4Songs=ts.getLatestSessionSongsV4(dir);out.v4Info=ts.getLatestSessionInfoV4(dir);
const messages=[ts.osc('/Ping'),ts.osc('/Status/Deck/Song/Title',ts.arg.i(0),ts.arg.s('Unicode 🎵')),ts.osc('/Status/Deck/Playhead',ts.arg.i(2),ts.arg.f(12.5),ts.arg.f(1.01),ts.arg.f(129.2)),ts.osc('/Authorize',ts.arg.b(Buffer.from([0,1,255,4])))];
out.osc=messages.map(message=>{const bytes=ts.encodeOsc(message);return {hex:bytes.toString('hex'),decoded:normalize(ts.decodeOsc(bytes)),frame:ts.frameOsc(message).toString('hex')}});
const reader=new ts.FrameReader();const all=Buffer.concat(messages.map(ts.frameOsc));let decoded=[];for(let i=0;i<all.length;i++)decoded.push(...reader.push(all.subarray(i,i+1)));out.framed=decoded.map(normalize);out.pendingBytes=reader.pendingBytes;
const remote=new ts.SeratoRemoteClient({peerName:'Synthetic Fixture'});const events=[];for(const name of ['deckChange','playhead','loopChange','mixerChange'])remote.on(name,payload=>events.push({type:name,payload}));
const dispatch=(address,...args)=>remote.handleStatus(ts.osc(address,...args));
dispatch('/Status/Deck/Song/Title',ts.arg.i(0),ts.arg.s('Old'));
dispatch('/Status/Deck/Song/Artist',ts.arg.i(0),ts.arg.s('Old Artist'));
dispatch('/Status/Deck/Song/Filepath',ts.arg.i(0),ts.arg.s('/Synthetic/old.mp3'));
await new Promise(r=>setTimeout(r,70));
dispatch('/Status/Deck/Song/Title',ts.arg.i(0),ts.arg.s('New'));
dispatch('/Status/Deck/Song/Artist',ts.arg.i(0),ts.arg.s('New Artist'));
dispatch('/Status/Deck/Song/Filepath',ts.arg.i(0),ts.arg.s('/Synthetic/new.mp3'));
await new Promise(r=>setTimeout(r,70));
dispatch('/Status/Deck/Playhead',ts.arg.i(0),ts.arg.f(12.5),ts.arg.f(1),ts.arg.f(128));
dispatch('/Status/Deck/Loop/AutoLoopOn',ts.arg.i(0),ts.arg.f(1));dispatch('/Status/Deck/Loop/BeatLength',ts.arg.i(0),ts.arg.f(4));dispatch('/Status/Deck/Loop/LoopRollOn',ts.arg.i(0),ts.arg.i(0));
dispatch('/Status/Video/Mixer/Crossfader',ts.arg.f(0.25));dispatch('/Status/Video/Deck/Mixer/Upfader',ts.arg.i(0),ts.arg.f(0.75));
dispatch('/Status/Deck/Song/Valid',ts.arg.i(0),ts.arg.f(0));out.remote=events;out.mixer=remote.getMixer();out.loop=remote.getDeckLoop(1);out.track=remote.getDeckTrack(1);await remote.stop();

const lifecycle=[];const connect=new ts.SeratoConnect({seratoPath:historyRoot,forceVersion:'v3',pollIntervalMs:60000});
for(const name of ['ready','deckChange','history','session'])connect.on(name,payload=>lifecycle.push({type:name,payload}));
await connect.start();clearInterval(connect.pollTimer);
const updated=Buffer.concat([...songs.map((s,i)=>record(i===0?{...s,played:1}:s)),record({title:'New Live',artist:'Live Artist',filePath:'/Synthetic/live.mp3',bpm:132,start:1700000200,played:1,deck:2})]);
fs.writeFileSync(path.join(historyRoot,'History/Sessions/2.session'),updated);fs.utimesSync(path.join(historyRoot,'History/Sessions/2.session'),new Date(),new Date(Date.now()+1000));await connect.poll();
const newSession=record({title:'New Session',artist:'Live Artist',filePath:'/Synthetic/session.mp3',bpm:134,start:1700000300,played:1,deck:3});fs.writeFileSync(path.join(historyRoot,'History/Sessions/3.session'),newSession);await connect.poll();connect.stop();
fs.writeFileSync(path.join(target,'updated.bin'),updated);fs.writeFileSync(path.join(target,'new_session.bin'),newSession);
out.v3Lifecycle=JSON.parse(JSON.stringify(lifecycle,(key,value)=>key==='seratoPath'?'synthetic':key==='date'?'DATE':value));
const net=require('net'),{once}=require('events');const listener=net.createServer();listener.listen(0,'127.0.0.1');await once(listener,'listening');const accepted=once(listener,'connection');const peer=net.connect(listener.address().port,'127.0.0.1');await once(peer,'connect');const [socket]=await accepted;
const remoteSession=new ts.RemoteSession({socket,subscribeTopics:['/Register/Status/Deck/Playhead'],peerName:'Synthetic Remote',peerUuid:'synthetic-peer-uuid'});remoteSession.on('error',()=>{});
const frames=[],framer=new ts.FrameReader();peer.on('data',bytes=>frames.push(...framer.push(bytes)));
for(const message of [ts.osc('/StreamMgmt/Authorize/Request',ts.arg.b(Buffer.from(Array.from({length:16},(_,i)=>i)))),ts.osc('/StreamMgmt/Pairing/Pair',ts.arg.s('Synthetic Serato'),ts.arg.s('Serato DJ'),ts.arg.i(0)),ts.osc('/Ping')]){peer.write(ts.frameOsc(message));await new Promise(r=>setTimeout(r,40));}
remoteSession.close();await new Promise(r=>setTimeout(r,40));out.sessionFrames=frames.map(normalize);peer.destroy();socket.destroy();await new Promise(r=>listener.close(r));
fs.writeFileSync(path.join(target,'expected.json'),JSON.stringify(out,null,2)+'\n');
const crypto=require('crypto');const generated=['schema.sql','session.bin','sessions.bin','updated.bin','new_session.bin','expected.json'];
fs.writeFileSync(path.join(target,'PROVENANCE.json'),JSON.stringify({source:'https://github.com/chrisle/serato-connect',commit:'8ce4aaf7642968fbde21ad413eff5b89db4debf4',license:'MIT',fixtures:'Entirely synthetic binary history, plaintext SQLite rows, OSC/nonce/peer/session fields; no user captures or audio.',oracle:'Actual checksum-verified pinned TypeScript; runtime only loopback TCP and injected/pure dispatch, never mDNS publication.',sha256:Object.fromEntries(generated.map(name=>[name,crypto.createHash('sha256').update(fs.readFileSync(path.join(target,name))).digest('hex')]))},null,2)+'\n');
console.log('Actual pinned TS runtime oracle: V3 binary/history, real plaintext V4 SQLite, OSC/framing, settled client events');
}finally{fs.rmSync(dir,{recursive:true,force:true});}})().catch(error=>{console.error(error);process.exitCode=1;});
