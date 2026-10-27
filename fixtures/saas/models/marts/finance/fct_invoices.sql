with refunds as (
    select ch.invoice_id, sum(r.amount_usd) as refunded_usd
    from {{ ref('stg_stripe__refunds') }} as r
    join {{ ref('stg_stripe__charges') }} as ch using (charge_id)
    group by 1
)

select
    i.invoice_id,
    c.account_id,
    i.subscription_id,
    i.status,
    i.amount_due_usd,
    i.amount_paid_usd,
    coalesce(r.refunded_usd, 0) as refunded_usd,
    i.amount_paid_usd - coalesce(r.refunded_usd, 0) as net_paid_usd,
    i.period_start,
    i.period_end,
    i.created_at
from {{ ref('stg_stripe__invoices') }} as i
left join {{ ref('stg_stripe__customers') }} as c using (stripe_customer_id)
left join refunds as r using (invoice_id)
