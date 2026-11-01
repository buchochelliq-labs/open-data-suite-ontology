select plan_id, product_name, amount_usd, billing_interval, interval_count, is_trial
from {{ ref('stg_stripe__plans') }}
