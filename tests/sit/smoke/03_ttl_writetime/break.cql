-- record4 is deleted from the target with a timestamp later than the migration wrote, then
-- rewritten on origin later still: the correction has to win on both counts.
DELETE FROM target.smoke_ttl_writetime USING TIMESTAMP 1087383780000000 WHERE key='record4';
INSERT INTO origin.smoke_ttl_writetime(key, t_col1) VALUES ('record4','AAAA') USING TTL 4000 AND TIMESTAMP 1087384200000000;
INSERT INTO origin.smoke_ttl_writetime(key,tw_col2) VALUES ('record4','BBBB') USING TTL 4000 AND TIMESTAMP 1087384200000000;
INSERT INTO origin.smoke_ttl_writetime(key, w_col3) VALUES ('record4','CCCC') USING TTL 4000 AND TIMESTAMP 1087384200000000;
INSERT INTO origin.smoke_ttl_writetime(key,   col4) VALUES ('record4','DDDD') USING TTL 4000 AND TIMESTAMP 1087384200000000;

-- record5: t_col1 is a TTL source only, so the change moves the TTL but not the writetime.
INSERT INTO origin.smoke_ttl_writetime(key, t_col1)  VALUES ('record5','A---') USING TTL 700000 AND TIMESTAMP 1087384200000000;
INSERT INTO origin.smoke_ttl_writetime(key, tw_col2) VALUES ('record5','B---') USING TTL 700000 AND TIMESTAMP 1087384200000000;

-- record6: w_col3 is a writetime source only, so the change moves the writetime but not the TTL.
INSERT INTO origin.smoke_ttl_writetime(key, w_col3)  VALUES ('record6','C---') USING TTL 700000 AND TIMESTAMP 1087384200000000;

-- record7: tw_col2 is both, so both move.
INSERT INTO origin.smoke_ttl_writetime(key, tw_col2) VALUES ('record7','B---') USING TTL 700000 AND TIMESTAMP 1087384200000000;

-- record8: t_col1 changes but no writetime source does, so the change is written with the old
-- timestamp and is lost. That is the case's point, and it is exact parity with Java.
INSERT INTO origin.smoke_ttl_writetime(key, t_col1)  VALUES ('record8','A---') USING TTL 700000 AND TIMESTAMP 1087384200000000;
