select
    id as hubspot_company_id,
    name as company_name,
    domain,
    cast(app_account_id as integer) as account_id,
    industry,
    employee_count
from {{ source('hubspot', 'hubspot_companies') }}
