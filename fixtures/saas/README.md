# Acme: a made-up B2B SaaS dbt project

The pilot fixture (#21): a subscription business with the structure and the
mess of a real one, so evaluation tasks look like questions a data team gets.

## Sources

- **Stripe** (billing): customers, plans, subscriptions, invoices, invoice
  lines, charges, refunds. A plan change cancels one subscription and starts
  another, as Stripe often records it.
- **App** (the product's database): accounts, workspaces, users, events,
  including logins, internal accounts and deleted accounts.
- **HubSpot** (CRM): companies, some never linked to a product account, and
  deals, including signed contracts that have not started.

## Models

27 models in five layers: `staging` (one per source table), `intermediate`
(account identity, subscription months, user activity), `marts` (finance,
core, product, sales), `legacy` (`mrr_legacy`, retired) and `reporting`
(`rpt_board_metrics`, `customer_health_scores`, neither documented in dbt).

The data reproduces a real defect: Globex's downgrade at the end of April
shows in `fct_mrr` as churn in April and reactivation in May.

## What the team knows (`.ontology/`)

- `concepts.yaml`: account, paying customer, MRR and its movements, active
  user and DAU, the money-columns standard, and the decision to retire
  `mrr_legacy`.
- `assets.yaml`: grains, which model represents or measures which concept, owners,
  and `fct_mrr`'s known issue (DATA-412).
- `finance.yaml` and `sales.yaml`: two ARR definitions that disagree, left
  unresolved.

Where dbt and the team differ (`dim_accounts`' description,
`fct_daily_active_users` counting any event), both are kept.

## Regenerating `target/`

`target/manifest.json` and `target/catalog.json` are committed so tests need
no dbt. After changing models or seeds, rebuild them with dbt and DuckDB:

```sh
cd fixtures/saas
python3 -m venv .venv && .venv/bin/pip install dbt-duckdb
export DBT_PROFILES_DIR=$PWD ACME_DUCKDB_PATH=/tmp/acme.duckdb
.venv/bin/dbt seed && .venv/bin/dbt build && .venv/bin/dbt docs generate
```

Then delete everything in `target/` except `manifest.json` and `catalog.json`.
