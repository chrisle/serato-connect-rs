CREATE TABLE history_session(id INTEGER PRIMARY KEY,start_time REAL,end_time REAL,name TEXT);
CREATE TABLE history_entry(id INTEGER PRIMARY KEY,session_id INTEGER,name TEXT,artist TEXT,deck TEXT,start_time REAL,end_time REAL,played INTEGER,bpm REAL,file_name TEXT,portable_id TEXT);
CREATE TABLE connection(location_id INTEGER,database_uri TEXT);
CREATE TABLE asset(location_id INTEGER,portable_id TEXT,type_specific_data TEXT,album TEXT,label TEXT,genre TEXT,key TEXT);
INSERT INTO history_session VALUES(1,1700000000,1700005000,'Old synthetic session'),(2,1700006000,NULL,'Current synthetic session');
INSERT INTO connection VALUES(1,'/Synthetic/Library/root.sqlite'),(2,'/Volumes/Synthetic/_Serato_/Library/location.sqlite'),(3,'/Synthetic/Library/beatport.sqlite');
INSERT INTO asset VALUES(1,'Synthetic/one.mp3',NULL,'Synthetic Album','Synthetic Label','House','Am'),(2,'Music/two.mp3',NULL,NULL,NULL,NULL,NULL),(3,'streaming://beatport/12345','{"cover_id":"https://example.test/synthetic-cover.jpg"}','Streaming Album','Streaming Label','Techno','Cm');
INSERT INTO history_entry VALUES(1,1,'Old','Fixture Artist','A',1700000001,1700000100,1,120,'old.mp3','Synthetic/old.mp3');
INSERT INTO history_entry VALUES(2,2,'Loaded','Fixture Artist','1',1700006001,0,0,128,NULL,'Synthetic/one.mp3'),(3,2,'External','Guest','B',1700006002,0,1,129,NULL,'Music/two.mp3'),(4,2,'Stream','Streaming Artist','C',1700006003,1700006004,1,130,NULL,'streaming://beatport/12345'),(5,2,'Orphan','',NULL,0,0,0,NULL,'fallback.mp3','missing-asset');
