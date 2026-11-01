select
    id as invoice_line_id,
    invoice_id,
    subscription_id,
    plan_id,
    amount_cents / 100.0 as amount_usd,
    is_proration,
    cast(period_start as date) as period_start,
    cast(period_end as date) as period_end
from {{ source('stripe', 'stripe_invoice_lines') }}
