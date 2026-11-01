select
    u.account_id,
    date_trunc('week', d.activity_date) as week_start,
    count(distinct d.user_id) as active_users,
    sum(d.core_action_count) as core_actions
from {{ ref('int_user_daily_activity') }} as d
join {{ ref('stg_app__users') }} as u using (user_id)
group by 1, 2
