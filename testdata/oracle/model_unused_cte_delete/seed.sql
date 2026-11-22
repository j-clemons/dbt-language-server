-- duplicates and NULLs, including a NULL key
CREATE OR REPLACE TABLE orders AS SELECT * FROM (VALUES
  (1, 'paid'),
  (1, 'paid'),
  (2, 'void'),
  (NULL, 'paid'),
  (3, NULL)
) AS v(id, status);
