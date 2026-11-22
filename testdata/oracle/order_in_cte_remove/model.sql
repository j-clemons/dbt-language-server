with s as (
  select id, k, note from t order by k desc nulls last
)
select id, k, note from s
