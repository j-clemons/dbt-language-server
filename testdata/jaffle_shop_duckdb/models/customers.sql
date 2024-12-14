with customers as (

    select * from {{ ref('stg_customers') }}

),

orders as (

    select * from {{ ref('stg_orders') }}

),

payments as (

    select * from {{ ref('stg_payments') }}

),

status as (

    select * from {{ ref('stg_customer_status') }}

),

customer_orders as (

        select
        customer_id,

        min(order_date) as first_order,
        max(order_date) as most_recent_order,
        count(order_id) as number_of_orders
    from orders

    group by customer_id

),

customer_payments as (

    select
        orders.customer_id,
        sum(amount) as total_amount

    from payments

    left join orders on
         payments.order_id = orders.order_id

    group by orders.customer_id

),

final as (

    select
        {{ var('jaffle_string') }} as jaffle_string,
        customers.customer_id,
        customers.first_name,
        customers.last_name,
        {{ full_name('first_name', 'last_name') }} as full_name,
        customer_orders.first_order,
        customer_orders.most_recent_order,
        customer_orders.number_of_orders,
        {{ times_five('number_of_orders') }} as lifetime_order_number,
        customer_payments.total_amount as customer_lifetime_value,
        {{ jaffle_package.add_values('number_of_orders', 'lifetime_order_number') }} as lifetime_order_number

    from customers

    left join customer_orders
        on customers.customer_id = customer_orders.customer_id

    left join customer_payments
        on  customers.customer_id = customer_payments.customer_id

)

select * from final
