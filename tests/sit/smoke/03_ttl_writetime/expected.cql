-- TTL is deliberately not selected. Java's expected.out records `ttl(col4)` and then works
-- around its own decay by truncating every line to 46 characters (alternateCheckResults.sh),
-- which makes the fixture assert a prefix of a number that changes with how long the suite
-- took to get here. The writetime is the part of FEA-040..046 that is deterministic, and it
-- is what this case asserts.
SELECT key, t_col1, tw_col2, w_col3, col4, writetime(col4) AS wt_col4 FROM target.smoke_ttl_writetime;
