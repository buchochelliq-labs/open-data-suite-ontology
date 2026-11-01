select
    id as user_id,
    cast(account_id as integer) as account_id,
    email,
    role,
    cast(created_at as timestamp) as created_at
from {{ source('app', 'app_users') }}
