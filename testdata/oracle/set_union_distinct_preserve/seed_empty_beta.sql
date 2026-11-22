-- beta stays empty
CREATE OR REPLACE TABLE alpha AS SELECT * FROM (VALUES
  (1), (1)
) AS v(id);
