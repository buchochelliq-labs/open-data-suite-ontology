with monthly as (
    select account_id, month_start, sum(monthly_amount_usd) as mrr_usd
    from {{ ref('int_subscription_months') }}
    group by 1, 2
),

months as (
    select distinct month_start from {{ ref('int_subscription_months') }}
),

spine as (
    select a.account_id, m.month_start
    from (select account_id, min(month_start) as first_month from monthly group by 1) as a
    join months as m on m.month_start >= a.first_month
),

filled as (
    select s.account_id, s.month_start, coalesce(m.mrr_usd, 0) as mrr_usd
    from spine as s
    left join monthly as m using (account_id, month_start)
),

compared as (
    select
        *,
        coalesce(lag(mrr_usd) over (partition by account_id order by month_start), 0) as previous_mrr_usd,
        max(mrr_usd) over (partition by account_id order by month_start rows between unbounded preceding and 1 preceding) as best_previous_mrr_usd
    from filled
)

select
    account_id,
    month_start,
    mrr_usd,
    previous_mrr_usd,
    mrr_usd - previous_mrr_usd as mrr_change_usd,
    case
        when previous_mrr_usd = 0 and coalesce(best_previous_mrr_usd, 0) = 0 and mrr_usd > 0 then 'new'
        when previous_mrr_usd = 0 and mrr_usd > 0 then 'reactivation'
        when previous_mrr_usd > 0 and mrr_usd = 0 then 'churn'
        when mrr_usd > previous_mrr_usd then 'expansion'
        when mrr_usd < previous_mrr_usd then 'contraction'
        else 'unchanged'
    end as change_category
from compared
where mrr_usd > 0 or previous_mrr_usd > 0
