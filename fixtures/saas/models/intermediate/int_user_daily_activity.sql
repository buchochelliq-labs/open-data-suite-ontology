-- One row per user per day with at least one event.
select
    user_id,
    cast(occurred_at as date) as activity_date,
    count(*) as event_count,
    count(*) filter (where event_name in ('project_created', 'report_viewed', 'query_run')) as core_action_count
from {{ ref('stg_app__events') }}
group by 1, 2
