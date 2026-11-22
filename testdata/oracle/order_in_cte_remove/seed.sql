-- NULLs and duplicates, so a sort that changed the bag would show.
CREATE OR REPLACE TABLE t AS SELECT * FROM (VALUES
  (1, 10, 'a'),
  (2, 20, 'b'),
  (2, 20, 'b'),
  (3, NULL, 'c'),
  (4, NULL, NULL)
) AS v(id, k, note);
