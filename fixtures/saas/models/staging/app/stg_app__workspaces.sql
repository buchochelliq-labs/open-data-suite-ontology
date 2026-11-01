select
    id as workspace_id,
    cast(account_id as integer) as account_id,
    name as workspace_name,
    cast(created_at as timestamp) as created_at
from {{ source('app', 'app_workspaces') }}
