select
    d.deal_id,
    d.hubspot_company_id,
    co.account_id,
    d.deal_stage,
    d.deal_stage = 'closedwon' as is_won,
    d.amount_usd,
    d.close_date,
    d.pipeline
from {{ ref('stg_hubspot__deals') }} as d
left join {{ ref('stg_hubspot__companies') }} as co using (hubspot_company_id)
