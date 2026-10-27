select
    w.account_id,
    w.week_start,
    least(100, w.active_users * 20 + w.core_actions * 10) as health_score
from {{ ref('fct_account_activity_weekly') }} as w
