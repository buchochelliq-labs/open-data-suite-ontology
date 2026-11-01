with current_subscriptions as (
    select distinct c.account_id
    from {{ ref('stg_stripe__subscriptions') }} as s
    join {{ ref('stg_stripe__customers') }} as c using (stripe_customer_id)
    join {{ ref('stg_stripe__plans') }} as p using (plan_id)
    where s.status = 'active' and not p.is_trial
)

select
    a.account_id,
    a.account_name,
    a.plan_tier,
    a.is_internal,
    a.deleted_at is not null as is_deleted,
    cs.account_id is not null as has_paid_subscription,
    i.stripe_customer_id,
    i.hubspot_company_id,
    a.created_at
from {{ ref('stg_app__accounts') }} as a
left join {{ ref('int_account_identity') }} as i using (account_id)
left join current_subscriptions as cs using (account_id)
