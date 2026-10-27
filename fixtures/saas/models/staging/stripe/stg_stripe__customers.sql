select
    id as stripe_customer_id,
    cast(account_id as integer) as account_id,
    email,
    cast(created_at as timestamp) as created_at
from {{ source('stripe', 'stripe_customers') }}
where livemode
