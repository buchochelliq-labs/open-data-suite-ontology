select
    activity_date,
    count(distinct user_id) as daily_active_users
from {{ ref('int_user_daily_activity') }}
group by 1
