select
    id as plan_id,
    product as product_name,
    amount_cents / 100.0 as amount_usd,
    billing_interval,
    interval_count,
    is_trial
from {{ source('stripe', 'stripe_plans') }}
