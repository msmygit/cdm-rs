DELETE FROM target.feature_explode_map WHERE key='key2';
UPDATE target.feature_explode_map SET value='value999' WHERE key='key3' AND fruit='apples';
UPDATE target.feature_explode_map SET fruit_qty=999 WHERE key='key3' AND fruit='oranges';
DELETE FROM target.feature_explode_map WHERE key='key3' AND fruit='kiwi';

INSERT INTO origin.feature_explode_map(key,value,fruits) VALUES ('key1','valueA', {'apples': 3, 'oranges': 5, 'bananas': 2, 'grapes': 11});
INSERT INTO origin.feature_explode_map(key,value,fruits) VALUES ('key2','valueB', {'apples': 4, 'oranges': 6, 'bananas': 3, 'pears': 7});
INSERT INTO origin.feature_explode_map(key,value,fruits) VALUES ('key3','valueC', {'apples': 5, 'oranges': 7, 'bananas': 4, 'kiwi': 42});
