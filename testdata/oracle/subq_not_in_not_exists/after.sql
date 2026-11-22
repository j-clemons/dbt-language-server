select o.id, o.customer_id
from orders as o
where not exists (select b.customer_id from blocked as b where o.customer_id = b.customer_id)
