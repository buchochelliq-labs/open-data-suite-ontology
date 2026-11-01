-- One row per product account, with its Stripe customer and HubSpot company.
select
    a.account_id,
    c.stripe_customer_id,
    h.hubspot_company_id
from {{ ref('stg_app__accounts') }} as a
left join {{ ref('stg_stripe__customers') }} as c on c.account_id = a.account_id
left join {{ ref('stg_hubspot__companies') }} as h on h.account_id = a.account_id
