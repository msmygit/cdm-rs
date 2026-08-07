DELETE FROM target.map_columns WHERE key_a=2;
UPDATE target.map_columns SET val_a='valueD' WHERE key_a=3;

INSERT INTO origin.map_columns(key_a, key_b, val_a, val_b) VALUES (1, 'key1','valueA', 21);
INSERT INTO origin.map_columns(key_a, key_b, val_a, val_b) VALUES (2, 'key2','valueB', 22);
INSERT INTO origin.map_columns(key_a, key_b, val_a, val_b) VALUES (3, 'key3','valueC', 23);
