DROP TABLE IF EXISTS origin.smoke_ttl_writetime;
CREATE TABLE origin.smoke_ttl_writetime(key text, t_col1 text, tw_col2 text, w_col3 text, col4 text, PRIMARY KEY (key));

INSERT INTO origin.smoke_ttl_writetime(key,t_col1) VALUES ('record1','AAAA') USING TTL 60000 AND TIMESTAMP 1087383600000000;
INSERT INTO origin.smoke_ttl_writetime(key,tw_col2) VALUES ('record1','BBBB') USING TTL 5000 AND TIMESTAMP 1087383660000000;
INSERT INTO origin.smoke_ttl_writetime(key,w_col3) VALUES ('record1','CCCC') USING TTL 400 AND TIMESTAMP 1087383720000000;
INSERT INTO origin.smoke_ttl_writetime(key,col4) VALUES ('record1','DDDD') USING TTL 700000 AND TIMESTAMP 1087383780000000;

INSERT INTO origin.smoke_ttl_writetime(key,t_col1) VALUES ('record2','AAAA') USING TTL 5000 AND TIMESTAMP 1087383600000000;
INSERT INTO origin.smoke_ttl_writetime(key,tw_col2) VALUES ('record2','BBBB') USING TTL 60000 AND TIMESTAMP 1087383720000000;
INSERT INTO origin.smoke_ttl_writetime(key,w_col3) VALUES ('record2','CCCC') USING TTL 400 AND TIMESTAMP 1087383660000000;
INSERT INTO origin.smoke_ttl_writetime(key,col4) VALUES ('record2','DDDD') USING TTL 700000 AND TIMESTAMP 1087383780000000;

INSERT INTO origin.smoke_ttl_writetime(key,t_col1) VALUES ('record3','AAAA') USING TTL 5000 AND TIMESTAMP 1087383600000000;
INSERT INTO origin.smoke_ttl_writetime(key,tw_col2) VALUES ('record3','BBBB') USING TTL 60000 AND TIMESTAMP 1087383660000000;
INSERT INTO origin.smoke_ttl_writetime(key,w_col3) VALUES ('record3','CCCC') USING TTL 400 AND TIMESTAMP 1087383720000000;
INSERT INTO origin.smoke_ttl_writetime(key,col4) VALUES ('record3','DDDD') USING TTL 700000 AND TIMESTAMP 1087383780000000;

INSERT INTO origin.smoke_ttl_writetime(key,t_col1) VALUES ('record4','AAAA') USING TTL 4000 AND TIMESTAMP 1087383600000000;
INSERT INTO origin.smoke_ttl_writetime(key,tw_col2) VALUES ('record4','BBBB') USING TTL 4000 AND TIMESTAMP 1087383600000000;
INSERT INTO origin.smoke_ttl_writetime(key,w_col3) VALUES ('record4','CCCC') USING TTL 4000 AND TIMESTAMP 1087383600000000;
INSERT INTO origin.smoke_ttl_writetime(key,col4) VALUES ('record4','DDDD') USING TTL 4000 AND TIMESTAMP 1087383600000000;

INSERT INTO origin.smoke_ttl_writetime(key,t_col1) VALUES ('record5','AAAA') USING TTL 5000 AND TIMESTAMP 1087383600000000;
INSERT INTO origin.smoke_ttl_writetime(key,tw_col2) VALUES ('record5','BBBB') USING TTL 60000 AND TIMESTAMP 1087383660000000;
INSERT INTO origin.smoke_ttl_writetime(key,w_col3) VALUES ('record5','CCCC') USING TTL 400 AND TIMESTAMP 1087383720000000;
INSERT INTO origin.smoke_ttl_writetime(key,col4) VALUES ('record5','DDDD') USING TTL 700000 AND TIMESTAMP 1087383780000000;

INSERT INTO origin.smoke_ttl_writetime(key,t_col1) VALUES ('record6','AAAA') USING TTL 5000 AND TIMESTAMP 1087383600000000;
INSERT INTO origin.smoke_ttl_writetime(key,tw_col2) VALUES ('record6','BBBB') USING TTL 60000 AND TIMESTAMP 1087383660000000;
INSERT INTO origin.smoke_ttl_writetime(key,w_col3) VALUES ('record6','CCCC') USING TTL 400 AND TIMESTAMP 1087383720000000;
INSERT INTO origin.smoke_ttl_writetime(key,col4) VALUES ('record6','DDDD') USING TTL 700000 AND TIMESTAMP 1087383780000000;

INSERT INTO origin.smoke_ttl_writetime(key,t_col1) VALUES ('record7','AAAA') USING TTL 5000 AND TIMESTAMP 1087383600000000;
INSERT INTO origin.smoke_ttl_writetime(key,tw_col2) VALUES ('record7','BBBB') USING TTL 60000 AND TIMESTAMP 1087383660000000;
INSERT INTO origin.smoke_ttl_writetime(key,w_col3) VALUES ('record7','CCCC') USING TTL 400 AND TIMESTAMP 1087383720000000;
INSERT INTO origin.smoke_ttl_writetime(key,col4) VALUES ('record7','DDDD') USING TTL 700000 AND TIMESTAMP 1087383780000000;

INSERT INTO origin.smoke_ttl_writetime(key,t_col1) VALUES ('record8','AAAA') USING TTL 5000 AND TIMESTAMP 1087383600000000;
INSERT INTO origin.smoke_ttl_writetime(key,tw_col2) VALUES ('record8','BBBB') USING TTL 60000 AND TIMESTAMP 1087383660000000;
INSERT INTO origin.smoke_ttl_writetime(key,w_col3) VALUES ('record8','CCCC') USING TTL 400 AND TIMESTAMP 1087383720000000;
INSERT INTO origin.smoke_ttl_writetime(key,col4) VALUES ('record8','DDDD') USING TTL 700000 AND TIMESTAMP 1087383780000000;

DROP TABLE IF EXISTS target.smoke_ttl_writetime;
-- gc_grace_seconds=1 so that the tombstone break.cql writes is purged immediately on a
-- single node; without it the deleted row shadows the correction.
CREATE TABLE target.smoke_ttl_writetime(key text, t_col1 text, tw_col2 text, w_col3 text, col4 text, PRIMARY KEY (key)) WITH gc_grace_seconds=1;
