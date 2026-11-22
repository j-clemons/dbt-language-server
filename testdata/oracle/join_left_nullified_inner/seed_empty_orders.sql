-- orders stays empty
CREATE OR REPLACE TABLE customers AS SELECT * FROM (VALUES
  (10, 'active')
) AS v(id, state);
