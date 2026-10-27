select
    id as subscription_id,
    customer_id as stripe_customer_id,
    plan_id,
    status,
    quantity,
    cast(started_at as timestamp) as started_at,
    cast(canceled_at as timestamp) as canceled_at
from {{ source('stripe', 'stripe_subscriptions') }}
