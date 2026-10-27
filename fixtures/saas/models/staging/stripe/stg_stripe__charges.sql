select
    id as charge_id,
    invoice_id,
    amount_cents / 100.0 as amount_usd,
    status,
    cast(created_at as timestamp) as created_at
from {{ source('stripe', 'stripe_charges') }}
