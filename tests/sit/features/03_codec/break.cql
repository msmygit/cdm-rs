DELETE FROM target.codec WHERE key='key2';
UPDATE target.codec SET val_int=654321, val_bigint=314159200000000, val_timestamp='2024-04-16 10:30:00+0000', val_decimal=999.1234 WHERE key='key3';

INSERT INTO origin.codec(key,val_int,val_bigint,val_timestamp,val_decimal,val_double) VALUES ('key1','1234'  ,'9223372036854775807','040616110000','3.14','21474836470.7');
INSERT INTO origin.codec(key,val_int,val_bigint,val_timestamp,val_decimal,val_double) VALUES ('key2','12345' ,'2147483648'         ,'990616110000','4.14','21474836470.7');
INSERT INTO origin.codec(key,val_int,val_bigint,val_timestamp,val_decimal,val_double) VALUES ('key3','123456','3141592653589793'   ,'990616110000','5.14','21474836470.7');
