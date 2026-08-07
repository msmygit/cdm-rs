DELETE FROM target.feature_explode_map_with_constants WHERE const1='abcd' AND const2=1234 AND key='key2';
UPDATE target.feature_explode_map_with_constants SET time=7398730800000 WHERE const1='abcd' AND const2=1234 AND key='key3' AND fruit='apples';
UPDATE target.feature_explode_map_with_constants SET fruit_qty=999      WHERE const1='abcd' AND const2=1234 AND key='key3' AND fruit='oranges';
DELETE FROM target.feature_explode_map_with_constants WHERE const1='abcd' AND const2=1234 AND key='key3' AND fruit='kiwi';

INSERT INTO origin.feature_explode_map_with_constants(key,time,fruits) VALUES ('key1','1087383600000', {'apples': 3, 'oranges': 5, 'bananas': 2, 'grapes': 11});
INSERT INTO origin.feature_explode_map_with_constants(key,time,fruits) VALUES ('key2','1087383600000', {'apples': 4, 'oranges': 6, 'bananas': 3, 'pears': 7});
INSERT INTO origin.feature_explode_map_with_constants(key,time,fruits) VALUES ('key3','1087383600000', {'apples': 5, 'oranges': 7, 'bananas': 4, 'kiwi': 42});
