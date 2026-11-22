-- orders stays empty, so no row can be NULL-extended either way
CREATE OR REPLACE TABLE customers AS SELECT * FROM (VALUES
  (10, 'active')
) AS v(id, state);
