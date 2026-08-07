DROP TABLE IF EXISTS origin.feature_constant_column_remove;
CREATE TABLE origin.feature_constant_column_remove(const1 int, key text, value text, const2 int, PRIMARY KEY (const1, key));
INSERT INTO origin.feature_constant_column_remove(const1, key, value, const2) VALUES (1, 'key1','valueA', 21);
INSERT INTO origin.feature_constant_column_remove(const1, key, value, const2) VALUES (1, 'key2','valueB', 22);
INSERT INTO origin.feature_constant_column_remove(const1, key, value, const2) VALUES (1, 'key3','valueC', 23);

DROP TABLE IF EXISTS target.feature_constant_column_remove;
CREATE TABLE target.feature_constant_column_remove(key text, value text, PRIMARY KEY (key));
