select
    id as event_id,
    user_id,
    workspace_id,
    event_name,
    cast(occurred_at as timestamp) as occurred_at
from {{ source('app', 'app_events') }}
