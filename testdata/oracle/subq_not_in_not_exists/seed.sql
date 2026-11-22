-- The witness: one NULL in blocked makes NOT IN never true, so the model
-- returns nothing while NOT EXISTS keeps the unblocked orders.
CREATE OR REPLACE TABLE orders AS SELECT * FROM (VALUES
  (1, 10),
  (2, 20),
  (2, 20),
  (3, 30)
) AS v(id, customer_id);

CREATE OR REPLACE TABLE blocked AS SELECT * FROM (VALUES
  (20),
  (CAST(NULL AS INTEGER))
) AS v(customer_id);
