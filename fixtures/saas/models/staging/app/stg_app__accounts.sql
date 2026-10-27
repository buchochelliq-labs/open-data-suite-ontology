select
    cast(id as integer) as account_id,
    name as account_name,
    plan_tier,
    is_internal,
    cast(created_at as timestamp) as created_at,
    cast(deleted_at as timestamp) as deleted_at
from {{ source('app', 'app_accounts') }}
