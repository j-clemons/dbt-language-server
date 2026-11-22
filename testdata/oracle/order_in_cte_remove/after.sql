with s as (
  select id, k, note from t
)
select id, k, note from s
