select o.id, o.customer_id, c.state
from orders as o
left join customers as c on o.customer_id = c.id
where c.state = 'active'
