select
    id as invoice_id,
    customer_id as stripe_customer_id,
    subscription_id,
    status,
    amount_due_cents / 100.0 as amount_due_usd,
    amount_paid_cents / 100.0 as amount_paid_usd,
    cast(period_start as date) as period_start,
    cast(period_end as date) as period_end,
    cast(created_at as timestamp) as created_at
from {{ source('stripe', 'stripe_invoices') }}
