-- beta stays empty
CREATE OR REPLACE TABLE alpha AS SELECT * FROM (VALUES
  (1), (2)
) AS v(id);
