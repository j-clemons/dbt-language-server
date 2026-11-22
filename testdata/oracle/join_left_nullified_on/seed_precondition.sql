-- The precondition: every order has an active customer, so moving the
-- predicate cannot change the result. Duplicates and NULLs stay off the key.
CREATE OR REPLACE TABLE orders AS SELECT * FROM (VALUES
  (1, 10, 'first'),
  (2, 10, NULL),
  (2, 10, NULL),
  (3, 20, 'second customer')
) AS v(id, customer_id, note);

CREATE OR REPLACE TABLE customers AS SELECT * FROM (VALUES
  (10, 'active'),
  (10, 'active'),
  (20, 'active')
) AS v(id, state);
