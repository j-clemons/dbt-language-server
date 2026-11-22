-- NULL keys and states, duplicate orders, a fanned-out customer, orphans
CREATE OR REPLACE TABLE orders AS SELECT * FROM (VALUES
  (1, 10, 'paid', 'first'),
  (2, 10, 'paid', NULL),
  (2, 10, 'paid', NULL),
  (3, 20, 'void', 'inactive customer'),
  (4, 30, 'paid', 'no such customer'),
  (5, NULL, 'paid', 'null key'),
  (6, 40, 'paid', 'null state')
) AS v(id, customer_id, status, note);

CREATE OR REPLACE TABLE customers AS SELECT * FROM (VALUES
  (10, 'active'),
  (10, 'active'),
  (20, 'inactive'),
  (40, NULL),
  (NULL, 'active')
) AS v(id, state);
