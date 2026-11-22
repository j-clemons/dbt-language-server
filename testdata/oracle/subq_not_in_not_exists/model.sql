select o.id, o.customer_id
from orders as o
where o.customer_id not in (select b.customer_id from blocked as b)
