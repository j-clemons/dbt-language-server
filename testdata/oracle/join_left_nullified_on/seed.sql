-- The witness: orders whose customer is missing, inactive or NULL survive
-- the moved predicate but not the original WHERE.
CREATE OR REPLACE TABLE orders AS SELECT * FROM (VALUES
  (1, 10, 'kept by both'),
  (2, 20, 'inactive customer'),
  (2, 20, 'inactive customer'),
  (3, 30, 'no such customer'),
  (4, NULL, NULL)
) AS v(id, customer_id, note);

CREATE OR REPLACE TABLE customers AS SELECT * FROM (VALUES
  (10, 'active'),
  (20, 'inactive')
) AS v(id, state);
