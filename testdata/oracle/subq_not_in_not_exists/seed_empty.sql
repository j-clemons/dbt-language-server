-- blocked stays empty, so NOT IN and NOT EXISTS keep every order alike,
-- even one with a NULL customer.
CREATE OR REPLACE TABLE orders AS SELECT * FROM (VALUES
  (1, 10),
  (4, CAST(NULL AS INTEGER))
) AS v(id, customer_id);
