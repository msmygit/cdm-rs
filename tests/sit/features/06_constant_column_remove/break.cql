DELETE FROM target.feature_constant_column_remove WHERE key='key2';
UPDATE target.feature_constant_column_remove SET value='value999' WHERE key='key3';

INSERT INTO origin.feature_constant_column_remove(const1, key, value, const2) VALUES (1, 'key1','valueA', 21);
INSERT INTO origin.feature_constant_column_remove(const1, key, value, const2) VALUES (1, 'key2','valueB', 22);
INSERT INTO origin.feature_constant_column_remove(const1, key, value, const2) VALUES (1, 'key3','valueC', 23);
