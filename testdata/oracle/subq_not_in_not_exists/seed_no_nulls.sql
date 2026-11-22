-- The precondition: no NULL on either side, duplicates included, and the
-- two forms agree.
CREATE OR REPLACE TABLE orders AS SELECT * FROM (VALUES
  (1, 10),
  (2, 20),
  (2, 20),
  (3, 30)
) AS v(id, customer_id);

CREATE OR REPLACE TABLE blocked AS SELECT * FROM (VALUES
  (20),
  (20)
) AS v(customer_id);
