select
    id as deal_id,
    company_id as hubspot_company_id,
    deal_stage,
    amount as amount_usd,
    cast(close_date as date) as close_date,
    pipeline
from {{ source('hubspot', 'hubspot_deals') }}
