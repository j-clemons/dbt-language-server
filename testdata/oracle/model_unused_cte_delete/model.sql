with dead as (select id from orders where status = 'void'),
live as (select id, status from orders)
select id, status from live
