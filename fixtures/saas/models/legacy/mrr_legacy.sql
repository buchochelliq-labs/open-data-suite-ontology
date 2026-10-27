-- Built for the 2025 board deck.
select
    s.subscription_id,
    c.account_id,
    date_trunc('month', s.started_at) as first_month,
    s.status,
    p.amount_usd * s.quantity as mrr_usd
from {{ ref('stg_stripe__subscriptions') }} as s
join {{ ref('stg_stripe__customers') }} as c using (stripe_customer_id)
join {{ ref('stg_stripe__plans') }} as p using (plan_id)
where s.status in ('active', 'trialing')
