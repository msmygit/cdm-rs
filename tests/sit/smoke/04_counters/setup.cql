DROP TABLE IF EXISTS origin.smoke_counters;
CREATE TABLE origin.smoke_counters(key text, col1 counter, col2 counter, PRIMARY KEY (key));
UPDATE origin.smoke_counters SET col1=col1+1,     col2=col2+2     WHERE key='record1';
UPDATE origin.smoke_counters SET col1=col1+10,    col2=col2+20    WHERE key='record2';
UPDATE origin.smoke_counters SET col1=col1+1000,  col2=col2+2000  WHERE key='record3';
UPDATE origin.smoke_counters SET col1=col1+10000, col2=col2+20000 WHERE key='record4';
UPDATE origin.smoke_counters SET col1=col1+500,   col2=col2+500   WHERE key='record5';
UPDATE origin.smoke_counters SET col1=col1+500                    WHERE key='record6';
UPDATE origin.smoke_counters SET col1=col1+1,     col2=col2+2     WHERE key='record7';

DROP TABLE IF EXISTS target.smoke_counters;
CREATE TABLE target.smoke_counters(key text, col1 counter, col2 counter, PRIMARY KEY (key));
-- record7 already exists on the target, so the migration must write the *delta* rather than
-- the value: getting this wrong leaves col1 at 2.
UPDATE target.smoke_counters SET col1=col1+1 WHERE key='record7';
