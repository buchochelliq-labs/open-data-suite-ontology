select
    u.user_id,
    u.account_id,
    u.email,
    u.role,
    a.is_internal,
    u.created_at
from {{ ref('stg_app__users') }} as u
left join {{ ref('stg_app__accounts') }} as a using (account_id)
