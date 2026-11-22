-- customers stays empty: every order is NULL-extended, the WHERE drops them all
CREATE OR REPLACE TABLE orders AS SELECT * FROM (VALUES
  (1, 10, 'paid', 'first'),
  (2, NULL, 'paid', NULL)
) AS v(id, customer_id, status, note);
