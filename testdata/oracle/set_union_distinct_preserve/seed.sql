-- duplicates within and across the arms, and a NULL in each
CREATE OR REPLACE TABLE alpha AS SELECT * FROM (VALUES
  (1), (1), (2), (NULL)
) AS v(id);

CREATE OR REPLACE TABLE beta AS SELECT * FROM (VALUES
  (1), (3), (3), (NULL)
) AS v(id);
