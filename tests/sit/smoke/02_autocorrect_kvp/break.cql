DELETE FROM target.smoke_autocorrect_kvp WHERE key='key2';
UPDATE target.smoke_autocorrect_kvp SET value='value999' WHERE key='key3';

-- Re-inserting on origin moves its writetime past the target's, so the correction is not
-- shadowed by the newer target cell it is replacing.
INSERT INTO origin.smoke_autocorrect_kvp(key,value) VALUES ('key1','valueA');
INSERT INTO origin.smoke_autocorrect_kvp(key,value) VALUES ('key2','valueB');
INSERT INTO origin.smoke_autocorrect_kvp(key,value) VALUES ('key3','valueC');
