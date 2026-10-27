select
    id as refund_id,
    charge_id,
    amount_cents / 100.0 as amount_usd,
    reason,
    cast(created_at as timestamp) as created_at
from {{ source('stripe', 'stripe_refunds') }}
