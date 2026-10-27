-- One row per paid subscription per calendar month in which it is active at month end.
with months as (
    select cast(range as date) as month_start
    from range(date '2026-01-01', date '2026-07-01', interval 1 month)
),

subscriptions as (
    select
        s.subscription_id,
        c.account_id,
        s.plan_id,
        s.quantity,
        s.started_at,
        s.canceled_at,
        p.amount_usd,
        p.billing_interval,
        p.is_trial
    from {{ ref('stg_stripe__subscriptions') }} as s
    join {{ ref('stg_stripe__customers') }} as c using (stripe_customer_id)
    join {{ ref('stg_stripe__plans') }} as p using (plan_id)
)

select
    s.subscription_id,
    s.account_id,
    m.month_start,
    s.plan_id,
    case when s.billing_interval = 'year' then s.amount_usd * s.quantity / 12 else s.amount_usd * s.quantity end as monthly_amount_usd
from subscriptions as s
join months as m
    on s.started_at < m.month_start + interval 1 month
    and (s.canceled_at is null or s.canceled_at >= m.month_start + interval 1 month)
where not s.is_trial
