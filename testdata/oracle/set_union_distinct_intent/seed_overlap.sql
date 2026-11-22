-- 1 repeats within alpha and across arms: UNION drops rows UNION ALL keeps
CREATE OR REPLACE TABLE alpha AS SELECT * FROM (VALUES
  (1), (1), (2)
) AS v(id);

CREATE OR REPLACE TABLE beta AS SELECT * FROM (VALUES
  (1), (3)
) AS v(id);
