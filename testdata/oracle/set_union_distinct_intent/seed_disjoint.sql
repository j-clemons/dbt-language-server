-- no value repeats anywhere, one NULL overall
CREATE OR REPLACE TABLE alpha AS SELECT * FROM (VALUES
  (1), (NULL)
) AS v(id);

CREATE OR REPLACE TABLE beta AS SELECT * FROM (VALUES
  (2), (3)
) AS v(id);
