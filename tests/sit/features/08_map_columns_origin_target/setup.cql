DROP TABLE IF EXISTS origin.map_columns;
CREATE TABLE origin.map_columns(key_a int, key_b text, val_a text, val_b int, PRIMARY KEY (key_a, key_b));
INSERT INTO origin.map_columns(key_a, key_b, val_a, val_b) VALUES (1, 'key1','valueA', 21);
INSERT INTO origin.map_columns(key_a, key_b, val_a, val_b) VALUES (2, 'key2','valueB', 22);
INSERT INTO origin.map_columns(key_a, key_b, val_a, val_b) VALUES (3, 'key3','valueC', 23);

-- The target drops the origin's clustering key and its `val_b`, and adds a `val_c` no origin
-- column feeds. Name-based mapping has to work all three out on its own (SCH-003, SCH-004).
DROP TABLE IF EXISTS target.map_columns;
CREATE TABLE target.map_columns(key_a int, val_a text, val_c int, PRIMARY KEY (key_a));
