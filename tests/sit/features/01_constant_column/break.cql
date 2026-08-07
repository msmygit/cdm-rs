DELETE FROM target.feature_constant_column WHERE key='key2' AND const1=1;
UPDATE target.feature_constant_column SET value='value999' WHERE key='key3' AND const1=1;

INSERT INTO origin.feature_constant_column(key,value) VALUES ('key1','valueA');
INSERT INTO origin.feature_constant_column(key,value) VALUES ('key2','valueB');
INSERT INTO origin.feature_constant_column(key,value) VALUES ('key3','valueC');
