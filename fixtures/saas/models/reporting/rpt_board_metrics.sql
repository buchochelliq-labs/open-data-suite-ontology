select
    month_start,
    sum(mrr_usd) as mrr_usd,
    sum(mrr_usd) * 12 as arr_usd,
    count(distinct case when mrr_usd > 0 then account_id end) as paying_accounts,
    sum(mrr_change_usd) as net_new_mrr_usd
from {{ ref('fct_mrr') }}
group by 1
