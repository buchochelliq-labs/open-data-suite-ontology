{# Load the CSV files in data/raw into the raw schema, as an ELT tool would. #}
{% macro load_raw() %}
    {% set tables = [
        'stripe_customers', 'stripe_plans', 'stripe_subscriptions', 'stripe_invoices',
        'stripe_invoice_lines', 'stripe_charges', 'stripe_refunds',
        'app_accounts', 'app_workspaces', 'app_users', 'app_events',
        'hubspot_companies', 'hubspot_deals'
    ] %}
    create schema if not exists raw;
    {% for t in tables %}
    create or replace table raw.{{ t }} as select * from read_csv_auto('data/raw/{{ t }}.csv', header = true);
    {% endfor %}
{% endmacro %}
