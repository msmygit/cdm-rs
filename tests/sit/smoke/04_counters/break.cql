UPDATE origin.smoke_counters SET col1=col1+1 WHERE key='record2';

-- CDM-29: a deleted counter row "zombies" — the delete is remembered but the row can still be
-- incremented back into existence. `fix` leaves it alone; `fixForce` recreates it.
DELETE FROM target.smoke_counters WHERE key='record3';

-- Changed by the same amount on both sides: still a match.
UPDATE origin.smoke_counters SET col2=col2+1 WHERE key='record4';
UPDATE target.smoke_counters SET col2=col2+1 WHERE key='record4';

-- Changed by different amounts: the target is brought back to the origin's value even though
-- that means counting down.
UPDATE origin.smoke_counters SET col2=col2+1 WHERE key='record5';
UPDATE target.smoke_counters SET col2=col2+2 WHERE key='record5';
