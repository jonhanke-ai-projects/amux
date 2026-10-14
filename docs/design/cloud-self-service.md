# amux Cloud: a hosting business amux runs by itself

Status: plan, revision 6. Owner: amux-cloud. Written 2026-10-14. Supersedes the
multi-tenant trial model in [cloud-poc-provisioning-plan.md](../cloud-poc-provisioning-plan.md).

**Purpose.** Turn amux into a small, profitable hosting business: a customer
pays, gets their own amux server, brings their own model key, and keeps paying
because it keeps working. amux operates the business: it sells, provisions,
supports, fixes and improves itself. The founder should be able to ignore it for
90 days and come back to more customers, more profit and fewer recurring
problems.

**Decision rule used throughout.** Prefer the simpler, more reliable system that
earns slightly less over a complex one that grows faster but needs active
management. Every design below was checked against it; anything that would add
founder work is flagged with **[owner]**.

**Evidence labels.** **[exists]** is verified in the repository or a live API
(file and line given). **[build]** is new work. **[H]** is a hypothesis to be
measured, never a fact.

## 1. Three parts, three failure domains

| Part | What it is | Runs on | Depends on |
|---|---|---|---|
| **amux OSS** | The product. Free, MIT plus Commons Clause, feature-complete [exists] | The user's machine | Nothing of ours |
| **amux Cloud** | Commercial hosting: control plane plus one server per customer | cloud.amux.io (control plane) and `<slug>.amux.cloud` | Stripe, Clerk, Hetzner, Cloudflare. **No amux agents.** |
| **amux Operator** | The internal business operator: workers, boards, schedules and MDAI that grow and maintain Cloud | Its own amux server (an ordinary amux install) | Read access to Cloud metrics; a narrow admin API |

The critical path (billing, sign-in, provisioning, emergency shutdown, recovery,
rollback) is plain code in the control plane and works with every Operator
worker stopped. The Operator can be frozen or deleted and Cloud keeps selling,
provisioning and serving customers. Customers' servers never depend on the
Operator.

## 2. Decisions

| # | Decision | Why |
|---|---|---|
| D1 | **Single tenant: one Hetzner server per paying customer.** | Each server pays for itself on day one; isolation is physical; "your own server" is easy to sell. The multi-tenant host is offline already (`.github/workflows/deploy-cloud.yml:229`, `CLOUD_HOST` unset) [exists]. Comparison in 4.4. |
| D2 | **Price floor is 2x server cost; the launch price is $39 Solo, set by value.** | 2x protects margin; it does not capture what a 24/7 agent server is worth. At $39 a customer contributes $24.79 a month after VAT and fees against $11.13 at $26 (4.2); $39 wins unless it converts under about 45% as many buyers as $26 [H]. Comparable products charge $20 to $25 plus usage for hosted agents (Devin Core, Cursor background agents, Replit) and $1 to $11 per app for generic open-source hosting (PikaPods, Elestio). amux sits between: a commodity server plus always-on agents. **[owner]** approves the launch price once; later changes need approval (section 11). |
| D3 | **Paid first, no free trial. The free tier is the open-source install, and the bridge is "move my local amux to a server".** | Re-evaluated against two alternatives (4.5). A trial spends a server on people who have not paid and invites abuse; a hosted preview has the same costs and support load. OSS users who already run amux are the warmest buyers, so the migration path is the funnel's centre (section 7). |
| D4 | **Bring your own model key or subscription.** We never hold or proxy model credentials. | No model spend on our books; the gateway's `/anthropic/` proxy (`cloud/gateway/gateway.py:1264`) is removed [exists]. |
| D5 | **One account: cloud.amux.io is the control plane** (Clerk accounts, Stripe billing, fleet database, provisioning, redirect). | Today Clerk lives on the gateway and passwords live in `cloud/desktop/door.py`; a customer would have two logins [exists]. |
| D6 | **Checkout first, account second.** | One form. The success URL creates the account idempotently (J5). |
| D7 | **EU by default; US later.** | The low-cost cx line exists only in fsn1, nbg1, hel1. In ash and hil the cheapest 8 GB server is ccx13 at $50.99 (shared cpx31 is $73.49), about 5x an EU server (Hetzner pricing API, 2026-10-14) [exists]. US is deferred until EU demand shows US customers churning on latency [H]. |
| D8 | **Cloudflare proxies every customer host.** | Cloudflare's edge certificate covers `*.amux.cloud` and its Origin CA issues per-host certificates with no rate limit, so Let's Encrypt limits (50 per registered domain and 5 duplicates per name per week) never apply. It also hides server IPs and absorbs floods. A managed service replaces three workarounds. Limit: 100 MB per request on the free plan; amux uploads already chunk (`upq` queue) [exists]. |
| D9 | **Releases ship as signed binary bundles.** | `AMUX_UPDATES=source` compiles Rust on the box for 10 to 15 minutes [exists]; wrong on a customer's machine. |
| D10 | **The objective is monthly owner cash flow, adjusted for owner time and risk.** Signups are an input, not a goal. | The business is judged on retained contribution profit at 30 and 90 days, owner minutes per month, and incidents resolved without the owner (section 9). |
| D11 | **Sell through a merchant of record (Paddle) rather than Stripe directly.** **[owner]** decides, because Stripe is already set up (O6). | A non-EU seller to EU consumers must register for VAT and file every quarter from the first sale, and must answer every card dispute. A merchant of record is the seller of record: it charges and files VAT, handles disputes and issues invoices. It also nets slightly more than Stripe once Stripe's 1.5% international-card and 1% currency fees are counted (4.2). Removing two recurring owner chores for less than nothing is exactly the decision rule. Stripe stays a fallback with Stripe Tax filing and automatic dispute evidence (M20). |
| D12 | **A server is running or deleted, never stopped.** | Hetzner bills stopped servers. Wherever a server would be parked (dunning, cancellation, a replaced unhealthy server) it is snapshotted and deleted; restoring is a re-provision from the snapshot in under 5 minutes [H]. Snapshots cost $0.0199/GB-month. |

## 3. Architecture

```
amux.io (static site)    GitHub README    App Store    amux OSS ("move to a server")
          \                    |              |                 /
           v                   v              v                v
cloud.amux.io  CONTROL PLANE (plain code, no agents)
   Clerk accounts | Stripe billing | fleet DB | provisioner | redirect | kill switches
   hcloud API (create, stop, snapshot, delete)  Cloudflare API (DNS, Origin CA)
          |                                                ^
          v                                                | read-only metrics, narrow admin API
<slug>.amux.cloud  (Cloudflare proxy)              amux OPERATOR (its own amux server)
   one Hetzner server per customer:                   workers, boards, schedules, MDAI
   Caddy (Origin CA) -> door (control-plane token)    growth, product, ops, finance loops
   -> amux-server -> workers, browser, files
   fleet agent: heartbeat, signed releases, backups
```

| Component | Exists | Build |
|---|---|---|
| Accounts | Clerk JWT check, user and org creation (`/api/cloud-auth`, `gateway.py:2522`); org invites (`:2611-2633`); referral codes (`/ref/`) | Account from Checkout (J5); an `email_verified` flag kept by the control plane (Clerk marks Backend-API users verified, so the restriction must be ours); remove `TRIAL_DAYS` (`:44`), trial rows (`:2539`, `:2683`) and the trial gate (`:3696-3707`) |
| Billing | Stripe checkout (`:3140`), webhook (`:2382`), portal (`:3203`), status (`:3223`) | If O6 picks a merchant of record: its checkout, webhooks and portal behind the same `billing` interface the control plane calls, so switching providers touches one module. Prices from `plans.json`; webhook and success URL create `provision_jobs`; handle `invoice.payment_failed` (only logged at `:2483`); remove the $5,000 pages (`:496`, `:586`) and the budget gate (`:3727-3739`) |
| Provisioning | `cloud/desktop/bootstrap.sh` builds a complete host; `remote.py` loads and verifies | hcloud and Cloudflare clients; `servers` and `provision_jobs` tables; a golden image built nightly from `bootstrap.sh` |
| Sign-in on the server | `door.py` (cookie, Basic, `X-Door-User`); member identity (`bootstrap.sh:317-358`) | Token mode: door verifies a 60-second Ed25519 JWT (aud = slug); its cookie is `__Host-`-prefixed and host-only. Audit every cookie amux-server sets for the same property |
| Updates | `self_update.rs`, the auto-builder | Fleet agent applies signed bundles; canary 5%, 50%, 100% gated on fleet health; SIGKILL-safe swap |
| Backups | litestream to R2 in the compose template | The same on dedicated hosts, plus Hetzner backups and a weekly restore drill |
| Model keys | `PATCH /api/settings/env` (`crates/amux-server/src/api/settings.rs:51`); `has_api_key` (`api/mod.rs:1095-1125`); cloud key modal (`app.js:1518-1530`) | A first-screen key step that validates with one provider call before saving |
| Analytics | PostHog on the site (`site/site.js`, project "amux") and in the gateway (`POSTHOG_KEY`, `gateway.py:21`) | The event taxonomy (section 6); the control plane mirrors every funnel event in its own table |
| Operator | Workers, board with evidence gates, scheduler (`kind: shell` costs nothing per run), MDAI with `amux:messages`, file and `fetch` sources, Messages channels, browser automation, cost tracking, `amux alert` (email only today) | An `amux-business` group on its own cx33 ($12.59/month); daily data exports from the control plane into its files; the narrow admin API |
| Experiments | A site experiment log with 76 status lines: 5 wins, 2 no effect, 50 inconclusive (mostly for lack of traffic) and 19 still marked running long after their window (`docs/homepage-experiments.md`) | M14 respects that evidence (no forced conclusions) and closes stale rows |

**Provisioning, simplified.** Cold path only at launch: create from the golden
image, cloud-init writes hostname, control-plane public key, owner email and
fleet token; Cloudflare DNS record and Origin CA certificate. Target paid to
ready under 3 minutes [H until measured]. No warm pool (section 13 says when to
add one).

**Synthetic customers and certificates.** Behind Cloudflare, a new synthetic
server per run issues no public certificate, so hourly runs are free of rate
limits.

## 4. Economics

### 4.1 Cost per server (measured, Hetzner API, USD/month excl. VAT, 2026-10-14)

| Plan | Server | Specs | Server | IPv4 | Backups 20% | Cost | 2x floor | Launch price [owner] |
|---|---|---|---|---|---|---|---|---|
| Solo | cx33 EU | 4 vCPU, 8 GB, 80 GB | $9.99 | $0.60 | $2.00 | $12.59 | $26 | **$39** |
| Team | cx43 EU | 8 vCPU, 16 GB, 160 GB | $18.49 | $0.60 | $3.70 | $22.79 | $46 | **$69** |
| Max | cx53 EU | 16 vCPU, 32 GB, 320 GB | $34.99 | $0.60 | $7.00 | $42.59 | $86 | **$129** |
| US (later) | ccx13 | 2 dedicated vCPU, 8 GB, 80 GB | $50.99 | $0.60 | $10.20 | $61.79 | $124 | **$149** |

Prices are shown including VAT to EU consumers and excluding it to businesses with a VAT number (reverse charge), as a merchant of record does by default; the tables below use the net.

### 4.2 Profit at scale

Variable per customer: server; payment fees (merchant of record: 5% + $0.50 all-in; Stripe: 2.9% + $0.30, Billing 0.7%, Tax 0.5%, plus 1.5% international and 1% currency conversion on the 75% of cards issued outside the US [H]); disputes 0.3% at $15 (Stripe path only) [H]; refunds 1% [H]; abuse 0.5% [H]; support inference $0.40 [H]; lapsed customers' snapshots about $0.02 [H]. VAT: 35% of customers are EU consumers at 21% [H]. Fixed: control plane ($7.09 per 1,500 servers [H]), the Operator's own server ($12.59), synthetic customers ($46: one started cx33 hour plus about $0.05 of tokens per hourly run [H]), Operator inference $250 (cap), Clerk beyond its free tier [H]. Owner time: target of no routine work, about 1 hour of exceptions per 1,000 customers at $150 [H]. Plan mix 80% Solo, 16% Team, 4% Max, EU only [H]. Computed by `docs/design/cloud-self-service-econ.py`; rerun it when an input changes.

Merchant-of-record path (recommended), monthly, USD:

| Ladder | Customers | Gross | Net of VAT | Variable | Fixed | Contribution | Margin | After owner time |
|---|---|---|---|---|---|---|---|---|
| $26 (2x cost) | 100 | $3,160 | $2,968 | $1,840 | $316 | $813 | 27% | $798 |
| $26 (2x cost) | 1,000 | $31,600 | $29,680 | $18,396 | $316 | $10,969 | 37% | $10,819 |
| $26 (2x cost) | 5,000 | $158,000 | $148,402 | $91,980 | $337 | $56,086 | 38% | $55,336 |
| **$39 entry** | 100 | $4,740 | $4,452 | $1,942 | $316 | $2,194 | 49% | $2,179 |
| **$39 entry** | 500 | $23,700 | $22,260 | $9,711 | $316 | $12,233 | 55% | $12,158 |
| **$39 entry** | 1,000 | $47,400 | $44,521 | $19,423 | $316 | $24,782 | 56% | $24,632 |
| **$39 entry** | 5,000 | $237,000 | $222,604 | $97,115 | $337 | $125,152 | 56% | $124,402 |
| $49 entry | 100 | $5,980 | $5,617 | $2,023 | $316 | $3,278 | 58% | $3,263 |
| $49 entry | 1,000 | $59,800 | $56,168 | $20,229 | $316 | $35,623 | 63% | $35,473 |
| $49 entry | 5,000 | $299,000 | $280,838 | $101,145 | $337 | $179,356 | 64% | $178,606 |

Stripe path at $39 for comparison: $24,475 contribution at 1,000 customers (against $24,782), plus quarterly VAT filing and every dispute as owner work. The script prints every combination.

### 4.3 Sensitivity: conversion and churn

Per 1,000 buyers at the $26 conversion rate, assuming $39 converts 0.80x and $49 0.65x as many [H]:

| Ladder | Monthly churn | Contribution per customer | Customers at day 90 | Contribution, months 1 to 3 | Lifetime contribution |
|---|---|---|---|---|---|
| $26 | 5% | $11.13 | 857 | $31,752 | $222,628 |
| $26 | 12% | $11.13 | 681 | $29,547 | $92,762 |
| $39 | 5% | $24.79 | 686 | $56,572 | $396,650 |
| $39 | 12% | $24.79 | 545 | $52,643 | $165,271 |
| $49 | 5% | $35.51 | 557 | $65,841 | $461,636 |
| $49 | 12% | $35.51 | 443 | $61,268 | $192,348 |

Reading it: a higher price wins on conversion alone unless conversion falls below about 45% ($39) or 31% ($49) of the $26 rate. **Churn decides it:** if $39 raised monthly churn from 5% to 12%, $26 at 5% would earn more over a customer's life ($222K against $165K). So the price is judged on 90-day retained contribution, not on payment rate (M13).

### 4.4 Single tenant against multi-tenant

| | Multi-tenant (today's gateway) | Single tenant (this plan) |
|---|---|---|
| Idle capacity | Ours; margin depends on packing | The customer's; priced per server |
| Noisy neighbour | Shared host; containers capped at 3 GB (`cloud/docker/docker-compose.template.yml`) | None |
| Blast radius of a bad release or escape | Every customer on the host | One customer, and canary rollout limits it further |
| Where keys and code live | A shared host we operate | The customer's own machine |
| Ops | One delicate host (`compose up --no-recreate`) | Many identical hosts, one fleet agent |
| Provisioning | Seconds | Under 3 minutes [H] |

### 4.5 No free trial, re-evaluated

| Option | Retained profit | Owner work | Abuse | Verdict |
|---|---|---|---|---|
| Paid first, OSS as the trial | Every server is paid for; buyers self-select | None added | Card on file before any server | **Chosen** |
| OSS user moves to Cloud (`amux cloud move`, section 7.1) | Warmest buyers: they already use amux; migration keeps their data, which raises retention [H] | None once built | None | **Chosen as the main bridge** (section 7) |
| Hosted preview (shared, time-limited) | Spends servers on non-buyers; preview-to-paid unknown | A second hosting mode to run and secure | High (free compute) | Rejected for now; revisit only if OSS installs stop growing |

### 4.6 Volume: how long organic traffic takes (measured 2026-10-14)

amux.io had 3,596 unique visitors in the last 30 days and 178 of them viewed `/pricing` (19 viewed `/cloud`) (PostHog). GitHub had 459 unique visitors in 14 days, with amux.io (177) and Google (64) the top referrers, and 534 stars since 2026-02-18 (about 67 a month). Months to 100 paying customers at 5% monthly churn:

| Share of pricing viewers who buy [H] | Pricing viewers flat | Pricing viewers +10%/month |
|---|---|---|
| 1% | over 60 months | 23 months |
| 2% | over 60 months | 17 months |
| 5% | 17 months | 9 months |
| 8% | 9 months | 7 months |

With flat traffic the business levels off where new customers equal churned ones: pricing viewers x buy rate / churn. At 5% monthly churn that is 36 customers at 1% ($588/month contribution at $39, after the fixed costs), 71 at 2% ($1,466), 178 at 5% ($4,152) and 285 at 8% ($6,837). **The 500 to 5,000 rows of 4.2 are reachable only if organic traffic grows severalfold**; they describe what the business is worth at scale, not when it gets there. A 2% to 8% pricing-to-paid rate for a paid-first $39 product is itself optimistic [H]; the 1% row is the planning case.

So the constraint is qualified traffic, not conversion tuning. The acquisition work (section 7) is measured by one number: pricing-page viewers per month from organic sources. Target: 178 today to 500 within 12 months (about +9% a month), tracked weekly in the report; content (M15), the move-to-Cloud CTAs (7.1) and README changes are judged by their share of that growth. Leading indicators, checked at 90 days: indexed pages and Search Console clicks that reach `/pricing`. If pricing viewers grow under 3% a month by then, the plan assumes the flat-traffic ceiling, content work stops, and the Operator's cap falls to its $50 floor.

### 4.7 Operating budgets (hard caps the Operator cannot exceed)

| Budget | Cap | Enforced by | Over the cap |
|---|---|---|---|
| Paid acquisition | $0 | No ad accounts exist; the Operator has no card | **[owner]** opens one deliberately |
| Operator inference | $50 plus 5% of last month's contribution, at most $250/month (about $70 at 36 customers) | A dedicated Anthropic Console workspace with a monthly spend limit (as used for sandbox.amux.io's key), reset monthly by the control plane | Workers stop at the limit; the weekly report shows what the Operator cost against the retained contribution its shipped changes are credited with, and the cap is not raised while that ratio is below 3x |
| Support-agent inference | $0.40 per customer per month, pooled | Same mechanism, its own workspace | Same |
| Infrastructure | Servers = paying subscriptions + synthetic + 1 | Hourly reconcile in the control plane (M5) | Provisioning freezes; email to owner |
| Refunds | Automatic up to the lesser of 2% of last month's revenue or $500 | Control plane counter | Further refunds queue for **[owner]**; unanswered after 7 days they are paid (the customer-friendly default) |
| Experiments | At most 2 running; at most 50% of traffic each | M14 | New experiments wait |

## 5. The Operator

**The dedicated monitoring worker.** The Operator is one worker, `amux-business`, whose standing job is to watch every signal the business produces and adjust what it is allowed to adjust. Collection is cheap and model-free: shell schedules snapshot each source into files every 15 minutes. The worker reads MDAI digests of those files every 4 hours and immediately on an anomaly, decides, acts within its autonomy level, and records what it did on its board.

| Source | What it reads | Collected | Adjusts on its own (level) | Reports or escalates |
|---|---|---|---|---|
| Payments (merchant of record or Stripe) | New, renewed, failed, refunded, disputed; MRR; plan mix | Every 15 min | Our own reminder email copy (L1). Dunning timing belongs to the provider and is not ours to change under a merchant of record. Refunds are issued by the control plane within the cap; the worker only recommends | Weekly cash line; refunds beyond the cap |
| Control plane | Provision jobs, server states, redirect outcomes, kill-switch state | Every 15 min | Re-runs a failed lifecycle step (L1) | Repeated failures as cards |
| Fleet usage | Value events, active weeks, CPU, memory, disk per server (counts only) | Every 15 min | Lifecycle messages M6, M7, M10 (L1); disk cleanup M3 (L1) | Activation and retention lines |
| Hetzner | Server list, pricing API, quota, the abuse@ mailbox | Hourly; pricing daily | Nothing directly: abuse isolation (M17) and quota cards run in the control plane; the worker reads and explains them | Price guard breaches (M13); deletions (L3) |
| PostHog | Funnel by stage, CTA clicks, drop-off pages, source attribution | Hourly | Copy and layout experiments within caps (M14, L2); content priorities (M15) | Funnel line; experiment results |
| Cloudflare | 5xx rate, latency, blocked requests per host | Every 15 min | Nothing; infrastructure faults go to M2 | Cards on sustained errors |
| GitHub | Stars, traffic, referrers, issues mentioning Cloud | Daily | README and release-note CTA wording (L1) | Acquisition line |
| App Store Connect | Installs, ratings, review text | Daily | Promotional text only (L1); description and keywords change only with a reviewed app version, so they are cards | Ratings below 4.0 as a card |
| Model providers' status pages | Anthropic and OpenAI incidents | Every 5 min | A banner on customer dashboards during an incident (L1) | None |
| Its own spend | Operator and support inference against their caps | Hourly | Slows itself at 80% (M18) | Budget line |

**Its credentials, each scoped to read unless stated** (a test asserts every write outside this list is refused, T-OP-1):

| Credential | Scope |
|---|---|
| Payments | Restricted read-only key (Stripe) or read-only API key (Paddle) |
| Hetzner | A read-only API token for each project |
| Cloudflare | Analytics read for the `amux.cloud` zone |
| PostHog | Personal API key with query access to the amux project only |
| GitHub | The App token (read), plus write to `cloud/`, `site/` and `docs/` via pull requests (section 5 merge scope) |
| App Store Connect | None. App Store Connect keys cannot be limited to one field, so the control plane holds the key and exposes a single endpoint that sets promotional text only |
| Email | Only the abuse@, support@ and privacy@ mailboxes, never the owner's |
| Control plane | The narrow admin API: read everything; write only re-run, restart and reminder-email actions |

It never touches the critical path (section 1): if it stops, collection keeps writing files and the control plane keeps running every customer-facing mechanism.

**Start small.** One Operator worker, `amux-business`, plus shell schedules. A
second worker is added only when the weekly report shows the first one's queue
older than 7 days for two consecutive weeks. The roles below are
responsibilities, not separate agents; they split into their own workers only
under that rule.

| Responsibility | Objective | Inputs | Tools | Permissions | Budget | Schedule | Output | Autonomy |
|---|---|---|---|---|---|---|---|---|
| Operations | Every customer server healthy; every lifecycle step completing | Fleet heartbeats, `provision_jobs`, backup results, synthetic runs | Control-plane admin API (restart, re-provision, refund within cap), amux board | Restart, re-run, roll back; never delete | Inside Operator inference | Every 15 min (shell checks) and on events | Resolved incidents; cards for the rest | L1 to L2 |
| Product and engineering | Fix what the funnel and synthetic tests show is broken | Failing tests, support themes, MDAI `failures` node | Repo, CI, PRs | Merge at L2 only under `cloud/`, `site/` and `docs/` when CI and the synthetic journey pass. Changes to the amux product itself are PRs that reach customers only through the normal release train with a 24-hour canary (M12); they never merge on the Operator's say-so | Inside Operator inference | Daily | Merged fixes with test evidence | L2 |
| Growth and PLG | More retained customers from organic channels | Funnel, attribution, search data, MDAI `content` and `experiments` nodes | Site repo, docs, browser automation for research | Publish docs and guides through CI quality gates; run experiments within caps | Inside Operator inference; $0 ads | Weekly | Pages, guides, experiments with results | L1 to L2 |
| Finance | Margin and budgets hold; billing reconciles | Stripe, Hetzner invoices and API, control-plane DB | Read-only Stripe and Hetzner, the price guard | None to move money; refunds only through the Operations cap | Nil | Daily | Reconciliation, the weekly cash-flow line | L1 |
| Support | Customers' problems resolved without the owner | Diagnostics bundles, server telemetry, docs | Per-customer support session (customer-approved), docs | Read diagnostics; run fixes the customer approves | $0.40/customer | On request | Answers, fixes, recurring-issue cards | L1 to L2 |

**Where the work runs.** Anything a customer depends on runs as plain code in the control plane and keeps working with the Operator frozen: provisioning recovery, self-healing, reconcile, releases, budgets, kill switches, abuse isolation and data requests (M1, M2, M3, M4, M5, M12, M17, M18, M19, M20). The Operator runs the improvement loops (M6 to M11, M13 to M16), which may stop without harming a customer. Section 10.0 lists each.

**Autonomy levels.**

| Level | Allowed | Examples |
|---|---|---|
| L0 Observe | Read and report | Weekly report, dashboards |
| L1 Reversible | Act alone when the change can be undone in one step | Restart a server, re-run provisioning, publish a doc page, fix a typo, run a retry |
| L2 Bounded | Act alone inside a budget or traffic cap | Refunds within the cap, experiments within the caps, merging a fix that passes CI and the synthetic journey |
| L3 Approval | Ask the owner, act only after a yes | Price changes, security incidents, legal or data requests, suspensions, anything that deletes customer data outside the documented lifecycle, raising a budget |

## 6. Funnel and event taxonomy

| Stage | Event | Key properties | Source of truth |
|---|---|---|---|
| Impression | `acq.impression` | channel, query, page | Search Console, GitHub traffic API, App Store Connect |
| Visit | `acq.visit` | source (`github`, `guide:<slug>`, `compare:<slug>`, `ios`, `cli`), utm | PostHog on amux.io |
| Qualified visit | `acq.qualified` | read pricing or docs for over 30 s, or copied the install command | PostHog |
| Install | `oss.install` | install method, anonymous install id (opt-in telemetry) | `install.sh` ping [build, opt-in] |
| Preview of value | `oss.first_worker` | install id | Opt-in OSS telemetry |
| Checkout | `cloud.checkout_started` | plan, region, source carried from the first visit | Control plane |
| Payment | `cloud.paid` | plan, amount, signup id | Stripe webhook |
| Ready | `cloud.ready` | ms from payment, path | Control plane |
| Technical first value | `cloud.first_reply` | ms from payment | Customer server reports to control plane |
| Activation (value) | `cloud.activated` | which value event | See below |
| Retention | `cloud.active_week` | weeks since payment, value events that week | Fleet agent counts, no content |
| Expansion | `cloud.upgraded`, `cloud.second_server` | from, to | Control plane |
| Referral | `cloud.referred_paid` | referrer org, retained 60 days | Control plane |
| Churn | `cloud.cancelled` | reason (optional, one question), tenure | Control plane |

Attribution: amux.io sets a first-touch source cookie; the plan button carries
it into Checkout metadata; the control plane stores it on the org. The OSS
dashboard's "Move to a server" link and `amux cloud` CLI command carry
`source=oss`. Every paid and retained customer is attributed to the page, guide,
repo or campaign that first brought them.

**Activation is a value event, not a reply.** Candidates [H], to be ranked by
their correlation with 90-day retention once 100 customers exist:

- V1: a worker's task produced an artifact the customer kept (a commit or PR on a connected repo, a file they opened or downloaded).
- V2: a schedule ran successfully three or more times.
- V3: work happened on two or more distinct days in the first week.
- V4: the phone app or a teammate connected.

Privacy: the fleet agent reports counts and event names only, never content,
prompts or code; the customer can see exactly what is sent in Settings.

Cohorts: weekly cohorts by first payment week, with D1, D7, D30 and D90
retention, value events per active week, plan changes, churn reasons and
lifetime contribution. Built by a shell schedule from the control-plane DB into
a file the MDAI `cohorts` node reads.

## 7. Acquisition: organic, compounding, low maintenance

| Channel | What exists | What to build | Why it compounds |
|---|---|---|---|
| GitHub README and repo | README, stars CTA experiments (EXP-001) [exists] | One line and badge: "Run it 24/7 on your own server" to `cloud.amux.io?source=github` | Every star is a qualified visitor |
| The OSS product itself | Self-hosted dashboard | "Move to amux Cloud" in Settings and `amux cloud move` in the CLI: bundle, pay, import in one flow (J12, section 7.1). Prompted only when the server shows it is losing work: the laptop slept with workers mid-task, or schedules missed while offline [H] | The user already has the habit; migration keeps their data |
| Docs, guides, compare pages | Large guide and compare set with an AEO backlog (`site/AEO_BACKLOG.md`, `site/compare/`, `site/guides/`) [exists] | The content pipeline (M15), with each page's paid and retained customers attributed | Pages keep ranking without upkeep |
| App Store | Listed (id6760410435) [exists] | Sign-in with amux Cloud; no purchasing in the app (11.O3) | Phone users become server buyers via the web |
| Integration tutorials | Skills, connectors, MCP [exists] | One tutorial per integration with a working, CI-tested example | High-intent searches ("run Claude Code 24/7") |

### 7.1 Move your local amux to Cloud

The warmest buyer already runs amux. Moving must take one command or one button, keep everything they built, and never surprise them.

**What exists.** `amux iac export` and `PUT /api/config/apply` turn an instance's configuration (skins, board columns, worker declarations) into YAML and back, idempotently, reporting what changed (`crates/amux-server/src/api/config_iac.rs`, `amux:5299-5354`) [exists]. `cloud/desktop/remote.py load ENV.yaml` loads workers, files and schedules onto a remote host and `verify` checks them; it is how sandbox.amux.io was loaded [exists]. Uploads already go in chunks (`upq`) [exists]. There is no data export today: `amux export` does not exist [build].

**The move bundle** [build], one file, `<name>.amuxmove`:

| Part | Source | On import |
|---|---|---|
| Configuration | `/api/config/export` (YAML) | `PUT /api/config/apply`, idempotent |
| Data | A per-table export of an allowlist: board cards and their history, schedules, memories, CRM, messages, MDAI runs, saved messages, skills. Written with the source's schema version. Never a raw copy of `amux.db`, which also holds secrets (`chatgpt_oauth_tokens`, migration 0098; share, member and invite tokens; connector state) | The importer loads it into a staging copy, runs migrations from the source's schema version, refuses any table not on the allowlist, then merges by stable ids; nothing on the server is overwritten without being shown in the diff |
| Workers | Each worker's env file (dir, tags, flags, model, description) | Declared, not started (the IaC rule); started from the import screen with one click |
| Files | `.mdai` nodes, skills, scratchpad, and any folder the user ticks | Restored under the same paths relative to home |
| Repositories | The remote URL and branch of each worker's checkout, not the checkout | Re-cloned on the server; uncommitted local changes are listed as a warning before the move, never silently left behind |
| Secrets | Excluded by default; the user may tick "bring my model keys" | Written with `PATCH /api/settings/env`; without the tick, J9 asks |

**Three ways in, one importer:**

1. **CLI:** `amux cloud move` makes the bundle, opens checkout (or uses the signed-in account), uploads in chunks while the server provisions, and opens the import screen when it is ready.
2. **OSS dashboard:** Settings, then "Move to amux Cloud": the same flow from the browser; the local server builds the bundle.
3. **New server's first screen:** "Import from your local amux" accepts a `.amuxmove` file or a plain `amux iac export` YAML.

The importer always shows a dry-run diff (workers, cards, schedules, files, what will be skipped) before applying, and the result screen lists every item imported or skipped with the reason. A move never deletes anything on the local machine.

**Where the call to action appears** (all [build] unless marked):

| Place | Call to action | Shown when |
|---|---|---|
| README, under the install command | "Run it 24/7 on your own server: `amux cloud move`" plus a badge linking `cloud.amux.io?source=github` | Always |
| GitHub repository About and website link | Points to `cloud.amux.io?source=github` | Always |
| Release notes on GitHub | One footer line: "Running amux on a laptop? `amux cloud move`" | Each release |
| `install.sh` closing message | "Want it running when your laptop sleeps? `amux cloud move`" | After a successful install |
| amux.io homepage | Secondary CTA beside "Get your own server": "Already running amux? Move it in one command" | Always |
| Pricing page | FAQ: "Can I bring my existing setup?" with the command | Always |
| Docs | A "Move to amux Cloud" page with the bundle contents table above | Always |
| OSS dashboard | Settings entry always; a one-time banner only when the server shows lost work (the machine slept with workers mid-task, or schedules missed while offline) [H] | As stated; dismissible, never repeated after dismissal |
| `amux cloud` CLI help | The move command first | Always |
| iOS app | None: no calls to buy in the app (O3) | Never |

Not used, by the decision rule: founder-led outbound, sales calls, bespoke
demos, consulting, and partnerships needing upkeep. The Calendly "Book a setup
call" on `site/cloud/index.html:50` is removed. An exception needs **[owner]**
and evidence that it pays more than its owner time.

## 8. The customer journey

Every stage has what the customer sees, what the system does, a target, the
automatic recovery and its test. Tests are in section 12.

| # | Stage | Customer sees | System does | Target | Automatic recovery | Test |
|---|---|---|---|---|---|---|
| J1 | Find us | amux.io, a guide, the README, the App Store, or the OSS dashboard | Every Cloud CTA points to `cloud.amux.io` with a source | Page LCP < 1.5 s | Static site on a CDN; a nightly link check fails the build on a dead CTA | T-SITE-1 |
| J2 | Compare plans | Free (open source) and Solo, Team, Max; one line on what they need: "Bring your Anthropic or OpenAI key, or a Claude or ChatGPT subscription" | Prices from `plans.json` | Interactive < 1 s | A price disagreeing with Stripe fails the deploy | T-SITE-2 |
| J3 | Choose | "Get your server" | `cloud.amux.io/start?plan=solo` | 1 click | Unknown plan falls back to Solo, logged | T-SITE-3 |
| J4 | Pay | Checkout from the merchant of record, or Stripe if O6 keeps it (Apple Pay, Google Pay on) | Session with a pending signup id and the source | < 60 s | Abandoned: one reminder after 1 hour if the customer consented in Checkout | T-PAY-1 |
| J5 | Account | Nothing to fill in | Success URL retrieves the session and, idempotently on its id, creates user, org and job (the webhook does the same; first wins); one-time sign-in token for the paying browser; `email_verified` false until email 1's link is clicked; other devices, invites and recovery need it | < 5 s | Existing account: payment attaches to it. Webhook late: success URL did the work | T-ACC-1, T-ACC-2, T-ACC-4 |
| J6 | Being built | Email 1 with the link | Control plane sends it | < 10 s | Bounce: shown on the progress page | T-MAIL-1 |
| J7 | Provision | Progress page with real steps, and what to have ready (links to the Anthropic and OpenAI key pages) | Create from the golden image, DNS, Origin CA certificate, health | < 3 min [H] | Capacity error: next location offering that type (from the pricing API). Over 15 min: refund within the cap, email 2, card | T-PROV-1, T-PROV-3, T-PROV-4, T-MAIL-2 |
| J8 | Enter | `https://<slug>.amux.cloud`, signed in | 60-second token, door `__Host-` cookie | < 2 s | Expired token: one bounce through the control plane | T-SSO-1 |
| J9 | Connect a model | First screen asks for a key, or "Move my local amux here" (J12) which brings their key | `PATCH /api/settings/env` on the customer's origin after one validating call; never via the control plane | < 1 min | Invalid key: provider's error, nothing saved | T-KEY-1..3 |
| J10 | First value | "Start with a template": code on a repo, inbox triage, research brief, a nightly schedule | Worker created, first task sent, reply streamed | Paid to first reply < 5 min [H] | Worker fails: the cause from its log, a retry | T-FIRST-1 |
| J11 | Make it theirs | Checklist: connect GitHub, phone app, teammate, a schedule | Items tick themselves | Each < 2 min | Inline errors; never blocks the dashboard | T-CHK-1..4 |
| J12 | Move local work | "Move to amux Cloud" in the OSS dashboard, `amux cloud move` in the CLI, or "Import from your local amux" on the new server's first screen (section 7.1) | A move bundle (configuration YAML plus a database snapshot and selected files) is created locally, uploaded in chunks, shown as a dry-run diff, then applied | < 10 min for 1 GB | Dry run first; apply is idempotent; a failed apply leaves the server as it was and lists what failed | T-MIG-1..5 |
| J13 | Teammate | Invite email, sign-in, lands in the server | Control-plane invite becomes an amux member with a scope | < 1 min | Invite bound to the address | T-TEAM-1 |
| J14 | Phone | App Store, "Sign in with amux Cloud", server listed | ASWebAuthenticationSession via the control plane | < 1 min | No server yet: progress page | T-IOS-1, T-IOS-2 |
| J15 | Day 2 and after | Release notes after an update | Fleet agent updates and backs up | Update downtime < 10 s | Bad release rolls back; failed backup retries, card after 2 | T-OPS-1..4 |
| J16 | Change plan | "Upgrade" or "Downgrade" | Upgrade: hcloud `change_type` with the larger disk, about 1 minute down, announced. Downgrade: a new smaller server and the J12 export and import (Hetzner cannot shrink a disk or restore a snapshot onto a smaller one) | Up < 5 min, down < 15 min | Failure restores the old server and reverses the change | T-PLAN-1, T-PLAN-3 |
| J17 | Payment fails | Banner with the provider's update-card link; emails 3 and 4 | Provider retries; on day 7 the server is snapshotted and deleted (D12) | The provider's schedule | A fixed card restores the server from its snapshot in under 5 minutes | T-DUN-1, T-MAIL-3, T-MAIL-4 |
| J18 | Cancel | "Cancel", with "Download everything" first and one optional reason; a recovery offer (M9) | Export; at period end the server is snapshotted and deleted; the snapshot is deleted after 30 days | Export < 10 min | Export failure blocks deletion | T-CAN-1, T-CAN-2, T-MAIL-5 |
| J19 | Delete my data | "Delete my account", or a data request by email (M19) | Server, snapshots, backups, R2, Clerk user, Stripe customer removed; receipt | < 24 h | Failed steps retried daily until done | T-DEL-1, T-MAIL-6 |
| J20 | Help | "Something wrong?" or an email to support@ | Diagnostics bundle the customer reads first; the support agent answers (M8) | First answer < 5 min | Unresolved: card with the bundle | T-HELP-1 |
| J21 | Another server | "Add a server" | Second subscription item and job; redirect shows a chooser | As J7 | As J7 | T-ADD-1 |
| J22 | Can't sign in | Email code, Google, GitHub, "I no longer have this email" | Clerk codes; the email-change path checks card last four and server slug | < 2 min with access | Rate-limited; notifies the old address | T-ACC-3 |

Deferred (section 13): region moves (they need export and import onto ccx13,
which cannot take a Team or Max disk), the US region, the warm pool.

**`cloud.amux.io` sends each visitor to one place.**

| Visitor state | Goes to | Test |
|---|---|---|
| Not signed in, no plan | `amux.io/pricing` | T-RED-1 |
| Not signed in, `?plan=` | Checkout for that plan | T-RED-2 |
| Signed out, has an account | Clerk sign-in, then this table again | T-RED-12 |
| Signed in, never paid | Pricing, signed in | T-RED-3 |
| Paid, provisioning | Progress page | T-RED-4 |
| Paid, ready | Their server with a fresh token | T-RED-5 |
| Several servers | A chooser, newest first | T-RED-6 |
| Payment failed, in grace | Their server with the banner | T-RED-7 |
| Past grace or cancelled | Billing: restart or download the export | T-RED-8 |
| Deleted | Pricing: "Your previous server was deleted on <date>" | T-RED-9 |
| Invited teammate | Sign-in, then the inviting org's server | T-RED-10 |
| `?from=ios` | The same, then back to the app | T-RED-11 |

**Emails** (control plane, one template set; Stripe sends receipts and invoices):
1 being built; 2 refunded because provisioning failed; 3 payment failed; 4 server
stopped for non-payment; 5 cancelled, with export link and deletion date;
6 deletion receipt. Lifecycle messages are in M6 to M10.

## 9. The owner's interface: exceptions only

**Weekly report** (email, Monday, generated by a shell schedule plus one MDAI
run; under one screen):

| Line | Measure |
|---|---|
| Cash | Revenue, contribution, owner-adjusted cash flow; 30 and 90-day retained contribution by cohort |
| Customers | New, churned, net; activation rate (V1 to V4); D7, D30 retention |
| Owner attention | Interventions per 100 active customers; owner minutes this week; unresolved exceptions; share of incidents resolved without the owner |
| Reliability | Synthetic pass rate, provisioning p95, failed backups, rollbacks |
| Operator | What it shipped, experiments started and concluded, budget used |
| Decisions waiting | Each L3 item: one-sentence question, what a yes does |

Owner minutes are measured, not estimated: every L3 approval, every alert the
owner opens and every card the owner touches is logged with a timestamp.

**Every escalation has an expiry and a safe default**, so an ignored question never blocks the business:

| Escalation | Default after 7 days unanswered |
|---|---|
| Refund beyond the cap | Refund it |
| Fleet mismatch over 3 servers (M5) | Stay frozen; existing customers unaffected |
| Restore drill failed (M4) | Keep retrying daily; take an extra Hetzner snapshot of every server weekly until it passes |
| Synthetic journey failing 6 hours (M11) | Roll back the last site, control-plane or release deploy |
| Price proposal (M13) | No change |
| Abuse appeal (M17) | Keep the server isolated, keep the data, refund the unused period |
| Data request from someone other than the account holder (M19) | Send the standard reply asking for verification through the account; disclose nothing |

The weekly report is written to be skimmed in a minute; it asks for nothing unless a line is in "Decisions waiting".

**What interrupts the owner** (email immediately; the phone channels are down,
see global notes): a security incident; customer data at risk; billing or
provisioning down for over 30 minutes after automatic recovery; a budget cap hit
in a way that stops customers; a legal or data request. Nothing else interrupts;
everything else waits for the weekly report.

**Release criterion for autonomy.** 30 consecutive days without a routine
owner intervention, then 90. Each routine intervention is filed as a defect with
the mechanism that should have handled it.

## 10. Closed-loop mechanisms

Ranked by expected retained profit times confidence, divided by effort plus
upkeep plus risk (section 10.1). Each runs Trigger, Data, Decision, Action,
Verification, Rollback, Escalation, and states its level (section 5).

| # | Mechanism | Trigger | Data | Decision | Action | Verification | Rollback | Escalation | Level |
|---|---|---|---|---|---|---|---|---|---|
| M1 | Provisioning recovery | Job step exceeds its time or errors | `provision_jobs`, Hetzner and Cloudflare errors | Retryable (capacity, timeout) or not | Retry in the next location; recreate from the image | `cloud.ready` within 15 min | Delete the half-built server | Over 15 min: refund within cap, email 2, card; over cap, **[owner]** | L2 |
| M2 | Self-healing server | Heartbeat missing 3 min, or health failing | Fleet heartbeats | Reachable (restart service) or not (reboot via API) | Restart amux; then reboot; then restore last backup to a new server | Health green 5 min | Snapshot the old server before deleting it; keep the snapshot 7 days (D12) | Third failure in a week: card | L1, restore L2 |
| M3 | Disk pressure | Disk over 85% | Fleet agent | Logs or caches (safe) or customer data | Prune logs, build caches, old snapshots; tell the customer if it is their data | Disk under 75% | Pruned items are regenerable | Customer data over 95%: offer the upgrade | L1 |
| M4 | Backup check | Backup missing or restore drill fails | Fleet agent, weekly drill | Transient or broken | Retry; rebuild litestream config | Next backup restores | n/a, read-only | Two failures: card; drill fails: **[owner]** | L1 |
| M5 | Billing and fleet reconcile | Hourly | Stripe subscriptions, `servers`, Hetzner server list | Mismatch type | Snapshot and delete servers with no paid subscription past grace; provision paid ones missing a server; snapshot and delete orphans older than 24 h | Counts match | Snapshots kept 30 days | Any mismatch over 3 servers at once: freeze provisioning, **[owner]** | L2 |
| M6 | Incomplete setup | 30 min after ready with no key, or 24 h with no first reply | Funnel events | Which step stalled | One email naming the step and the fix; in-app checklist highlight | The step completes | n/a | None; counted in the report | L1 |
| M7 | Inactivity | Paid, no value event for 7 days | `cloud.active_week` | Has the customer ever activated | One email with the template most used by retained customers; max one per 14 days | Value event within 7 days | n/a | None | L1 |
| M8 | Support resolution | "Something wrong?" or a message to support@ (read through the email API) | Diagnostics bundle, telemetry, docs | Known fix, safe fix, or unknown | Answer; apply a customer-approved fix | Customer marks resolved or no repeat in 7 days | Fixes are restarts or config edits with a backup | Unknown after the agent: card; data or billing disputes: **[owner]** | L2 |
| M9 | Cancellation recovery | Cancel clicked | Reason, usage | Reason is fixable (too expensive, missing feature, broke) | Offer the matching remedy once: downgrade, a fix, a pause | Stays 30 days | Customer can still cancel in one click | None | L1 |
| M10 | Expansion prompt | CPU or memory over 80% for 3 days, or a second team member active | Fleet agent | Would an upgrade remove the pressure | One in-app suggestion with the price difference | `cloud.upgraded` or pressure gone | Downgrade is one click | None | L1 |
| M11 | Synthetic failure to fix | Hourly journey fails | Step, screenshots, logs | Infra (M1, M2) or code | Card to Product; Product opens a PR | The failing test passes against the fix in test mode | Revert the PR | Fails 6 hours running: **[owner]** | L2 |
| M12 | Release rollout | New release | Fleet health | Canary healthy | 5%, 50%, 100% | Health after each stage | Fleet agent rolls back | Two rollbacks in a row: freeze releases, card | L2 |
| M13 | Price guard and price review | Daily guard; a review only when 90-day cohort data supports a change with confidence 0.8 or more | Hetzner pricing API; cohort contribution | Any plan below 1.95x cost; a price change justified by 90-day contribution | Card; for review, a written proposal | Proposal includes 4.3's churn test | n/a | Always **[owner]** for any price change | L0, L3 |
| M14 | Experiments | A hypothesis from the MDAI register; daily sweep of the log | Funnel, cohort, prior experiments | Enough traffic? Minimum 200 visitors and 10 payments per arm in the window. Below it: a reversible copy or layout change ships as "shipped, unmeasured" on judgment with guardrails watched, and is never reported as a result. Rows still "running" 28 days after their window are closed as inconclusive (the 19 stale rows first) | Variant behind a flag, traffic split within caps | Two-proportion test p < 0.05 on the primary metric; guardrails: refunds, churn, support load not worse | Flag off | A guardrail breach: off and card | L2 |
| M15 | Content pipeline | Weekly | Search Console queries, existing pages, the MDAI content register, attribution | Query relevant, no existing page on it, competition beatable | Draft; every code example runs in CI; every factual claim links its source; publish | Indexed; examples pass; qualified visits after 30 days | Unpublish automatically after 60 days with no qualified visit | None | L2 |
| M16 | Referral reward | Referred customer retained 60 days | Control plane | Real, paying, not self-referral | One month credit to the referrer | Credit applied | Reversed on chargeback | Fraud signals: **[owner]** | L2 |
| M17 | Abuse and fraud | Hetzner abuse email (read through the email API), sustained 100% CPU over 6 h, outbound spam signals, known mining processes, card-testing pattern at checkout | Fleet agent, payment provider's fraud signals, the abuse email | Clear abuse (a Hetzner report, or a known miner), likely, or noise | Clear: isolate (Hetzner firewall to deny outbound, workers stopped), tell the customer why and how to appeal, reply to Hetzner with the action taken, all before Hetzner's deadline. Likely: throttle CPU and notify. Checkout pattern: block it | Behaviour stops; Hetzner closes the report | Lift the firewall on a successful appeal | Deleting a customer's server or data: **[owner]**, never automatic; appeals with no reply default as in section 9 | L2; deletion L3 |
| M19 | Data requests | "Delete my account", "Download my data", or a data request to privacy@ | The account and its resources | Request from the account holder (verified email) or someone else | Run the J18 export or J19 deletion and send the receipt | Every listed resource exported or gone | Exports are additive; deletion is the request | Requests not from the account holder, or from authorities: **[owner]** | L2 |
| M20 | Payment disputes (Stripe path only) | A dispute opened | Provisioning log, usage counts, terms acceptance, invoices | Contest or accept (accept under $10) | Submit evidence through the provider's API | Dispute closed | n/a | None; counted in the report | L2 |
| M18 | Budget guard | Any budget in 4.7 reaches 80% | Spend counters | Projected to exceed | Slow down (fewer runs) at 80%; stop at 100% | Spend under cap | Resume next period | Stop that hurts customers: **[owner]** | L1 |

### 10.0 Where each mechanism runs, its status and its test

| # | Runs in | Status | Test |
|---|---|---|---|
| M1 Provisioning recovery | Control plane | [build] | T-M1 |
| M2 Self-healing | Control plane and fleet agent | [build] | T-M2 |
| M3 Disk pressure | Fleet agent | [build] | T-M3 |
| M4 Backup check | Fleet agent | [build]; litestream config [exists] | T-M4 |
| M5 Reconcile | Control plane | [build] | T-M5 |
| M6 Incomplete setup | Operator | [build] | T-M6 |
| M7 Inactivity | Operator | [build] | T-M7 |
| M8 Support | Operator, with the support workspace | [build]; diagnostics endpoints partly [exists] | T-M8 |
| M9 Cancellation recovery | Control plane offer, Operator copy | [build] | T-M9 |
| M10 Expansion prompt | Fleet agent signal, control plane offer | [build] | T-M10 |
| M11 Synthetic failure to fix | Operator; the synthetic run itself is a control-plane schedule | [build]; browser suites exist in `cloud/desktop/*.mjs` [exists] | T-M11 |
| M12 Release rollout | Control plane and fleet agent | [build]; `self_update.rs` [exists] | T-OPS-1, T-OPS-2, T-OPS-4 |
| M13 Price guard | Control plane schedule (guard), Operator (proposal) | [build] | T-M13 |
| M14 Experiments | Operator, through site flags | Experiment log [exists]; flags and the rules [build] | T-M14 |
| M15 Content | Operator | AEO and guide pages [exists]; pipeline [build] | T-M15 |
| M16 Referrals | Control plane | Referral codes [exists] (`/ref/`); retained-only reward [build] | T-M16 |
| M17 Abuse | Control plane | [build] | T-M17 |
| M18 Budget guard | Control plane and provider spend limits | Console spend limits [exists]; counters [build] | T-BUD-1 |
| M19 Data requests | Control plane | [build] | T-DEL-1, T-M19 |
| M20 Disputes | Control plane | [build] | T-M20 |

### 10.1 Prioritizing work

Every opportunity in the MDAI `opportunities` node gets five numbers: expected
change in 90-day retained contribution per month (dollars), confidence (0 to 1),
effort (agent-days), upkeep (owner minutes per month it will add), and risk (0
to 1). Score = (dollars x confidence) / (effort + upkeep / 60 + 10 x risk). The
Operator works the top item; anything with non-zero owner upkeep needs
justification in the card. Vanity metrics (stars, visits, signups) appear only
as inputs to the dollar estimate.

### 10.2 MDAI as the business's memory

| Node | Sources (all real MDAI source kinds) | Answers |
|---|---|---|
| `funnel.mdai` | Daily export file from the control plane | Where people stop, this week against last |
| `cohorts.mdai` | Cohort export file | Retention, activation, lifetime contribution |
| `experiments.mdai` | `docs/homepage-experiments.md` and new results | What has been tried; prevents repeats (the log already has 76) |
| `failures.mdai` | Synthetic results, support themes, incident cards | Recurring problems and their status |
| `pricing.mdai` | Hetzner pricing API (`fetch:`), cohort file | Margin and the price hypotheses |
| `decisions.mdai` | `amux:messages` from the owner, this plan | What was decided and why |
| `opportunities.mdai` | All of the above as upstream nodes | The ranked list in 10.1 |

## 11. Kill switches, risks and open decisions

**Kill switches** (control-plane flags plus the scheduler's enable API; each
leaves customer servers and data untouched):

| Freeze | Mechanism | Still works |
|---|---|---|
| Experiments | `experiments_enabled=false`: every flag serves control | Everything else |
| Deployments | `deploy_freeze=true`: CI refuses site and control-plane deploys; fleet agent ignores new releases | Customers on the current release |
| Spending | `spend_freeze=true`: Operator workspaces' limits set to 0, refunds queue | Customers, billing |
| Provisioning | `provisioning_freeze=true`: checkout shows "back soon", existing servers untouched | Existing customers |
| The Operator | Scheduler disabled for the `amux-business` group and its workers stopped (`amux stop`) | All of Cloud (section 1) |
| Everything new | All of the above at once | Existing customers keep their servers and data |

**Open decisions [owner]**

| ID | Question | Recommendation |
|---|---|---|
| O1 | Launch price | $39 Solo, $69 Team, $129 Max; review at 90 days of cohorts (M13) |
| O2 | Annual plans | None at launch |
| O3 | iOS purchasing | None. A free companion app to a paid web service needs no in-app purchase if it has no purchasing and no calls to buy (App Store guideline 3.1.3(f)); answer review questions with it |
| O4 | Existing subscribers ($20 monthly, $200 annual) and open "no credit card" trials (`site/pricing/index.html:239-275`, `:257`) | Read-only list in A0; owner decides grandfathering before the trial copy is removed |
| O5 | Customer domain | `amux.cloud`, behind Cloudflare |
| O6 | Payments: merchant of record (Paddle) or the existing Stripe setup | Merchant of record (D11): it nets slightly more and removes VAT filing and disputes |

**Risks**

| Risk | Mitigation |
|---|---|
| Hetzner per-project server limit, and a whole project locked over one customer's abuse | **[owner]** one-time increase request in A0; customers spread over two projects once there are 50 customers (a second one-time quota request) so one lock reaches at most half; M17 answers abuse reports before Hetzner's deadline; the provisioner files a card at 80% of quota |
| Cloudflare free-plan limits (100 MB request, 100 s response timeout) | amux uploads chunk; SSE pings every 10 s keep streams alive; long jobs are already asynchronous |
| Subscription sign-in (Claude, ChatGPT) on a headless server | Provider-dependent; the key path always works alone |
| Activation and price hypotheses wrong | Both are measured (section 6, M13) before anything depends on them |
| The Operator makes a bad change | L1 changes are one-step reversible; L2 changes need tests and caps; deploy freeze and rollback exist (above) |

| Legal pages and data processing terms | Terms, privacy policy, data processing agreement and subprocessor list (Hetzner, Cloudflare, Clerk, the payment provider, PostHog) written once in A0 from standard templates; a merchant of record supplies its own buyer terms |

**Designs that add founder work, and their mitigation:** A0's one-time setup
(accounts, quota request, launch price) **[owner]**, done once; price changes and
suspensions stay L3 on purpose; everything else is designed to need none.

## 12. Tests

One Playwright journey, `cloud/tests/journey.spec.ts`, drives a synthetic
customer from amux.io to a working server; the capability suite then runs on
that server. Every test asserts twice (the object exists in the API, and the UI
shows it). A capability that cannot run reports "not run" and fails the suite.

| ID | Asserts |
|---|---|
| T-SITE-1 | Every Cloud CTA (site, README, release notes, `install.sh`, OSS dashboard, CLI help; section 7.1) resolves to `cloud.amux.io` with its source or to `amux cloud move`; no Calendly or "free trial" anywhere |
| T-SITE-2 | Pricing page prices equal `plans.json` and live Stripe prices |
| T-SITE-3 | Each plan button lands in Checkout preselected |
| T-RED-1..12 | Each `cloud.amux.io` row, with an account placed in that state |
| T-PAY-1 | Card 4242 completes; one provision job; a replayed webhook makes none |
| T-ACC-1 | A new email is signed in after payment with no form |
| T-ACC-2 | An existing account's payment attaches to it |
| T-ACC-3 | Email-code sign-in; the lost-email path moves the account only with the right card digits and slug |
| T-ACC-4 | Success URL before webhook: one user, one job; another device refused until the email link |
| T-MAIL-1..6 | Each email arrives in the test inbox with working links |
| T-PROV-1 | Ready under 3 minutes |
| T-PROV-3 | Injected capacity error falls over to the next location offering the type |
| T-PROV-4 | Injected 15-minute failure refunds within the cap, emails and files the card |
| T-SSO-1 | Signed in; the token and cookie are rejected on another customer's server |
| T-KEY-1 | Key saved on the server origin, validated, absent from every control-plane log and table |
| T-KEY-2 | Invalid key: provider error, not saved |
| T-KEY-3 | OpenAI key path |
| T-FIRST-1 | Template worker replies within 60 s; homepage to first reply under 5 minutes |
| T-CHK-1..4 | GitHub, phone link, teammate, schedule each tick their item |
| T-MIG-1 | `amux cloud move` from a seeded local instance: the dry-run diff lists every worker, card, schedule, memory and file, then apply makes them present on the server |
| T-MIG-2 | Importing the same bundle twice changes nothing the second time and says so |
| T-MIG-3 | Secrets are absent unless "bring my model keys" was ticked: the bundle is scanned for token-shaped values (`sk-`, `ghp_`, OAuth and session tokens) and any match fails the test; with the tick the key works |
| T-MIG-4 | A worker with uncommitted local changes is flagged before the move; its repository is re-cloned on the server |
| T-MIG-5 | A plain `amux iac export` YAML imports on the first screen; a corrupt bundle, or one from an unknown schema version or with a table off the allowlist, is refused with the reason and changes nothing |
| T-OP-1 | Each Operator credential is used to attempt a write outside its scope; every attempt is refused |
| T-TEAM-1 | Invited account lands with its scope; a different email is refused |
| T-IOS-1 | XCUITest: sign in, server listed, dashboard loads |
| T-IOS-2 | XCUITest: message a worker, see the reply |
| T-OPS-1 | A release reaches the synthetic server; `/health` shows it |
| T-OPS-2 | A broken release rolls back |
| T-OPS-3 | Restore drill matches data |
| T-OPS-4 | Fleet agent SIGKILLed mid-swap: old or new version, never neither, and it says which |
| T-PLAN-1 | Upgrade keeps data |
| T-PLAN-3 | Downgrade by export and import; too much data names the space to free |
| T-DUN-1 | Test clock: failed renewal stops the server on day 7; new card restarts it |
| T-CAN-1 | Export downloads and opens |
| T-CAN-2 | Test clock past 30 days: server and snapshot gone |
| T-DEL-1 | Every listed resource gone; receipt lists each |
| T-HELP-1 | Bundle produced; agent answers |
| T-ADD-1 | Second server for the org, in the chooser |
| T-M1..T-M20 | One per mechanism (section 10.0): inject its trigger in test mode, assert its action, verification, rollback and escalation each happen |
| T-KILL-1 | Each kill switch freezes its target and leaves a customer server serving; with the Operator frozen, M1, M2, M5, M17 and M18 still act |
| T-BUD-1 | A budget at 100% stops its spender and nothing else |

**Capability suite** (on the synthetic server after T-FIRST-1; test accounts and
bots from A0, checked monthly by the suite):

| Tab or capability | Test |
|---|---|
| Workers | A Claude and a Codex worker each answer a task |
| Board | A card moved to done with evidence |
| Groups | Group created, worker added, filter works |
| Projects | Project on a test repo, checkout present |
| Scheduler | Shell schedule run now, run recorded |
| Files | Upload, preview, rename, delete |
| Scratchpad | Paste saved |
| MDAI | Node over `amux:messages` runs; tokens on the run |
| Messages | @mention in a channel; worker replies there |
| Skills | A skill example runs |
| Browser | Page opened, screenshot taken |
| Email | Test inbox: newest message read, draft queued for approval |
| Calendar | Event created, present in the iCal feed |
| Connectors | Test credential saved and removed |
| Draw | Shape survives a reload |
| Record | 5-second audio uploaded, listed |
| Map | Location point posted, rendered at its coordinates |
| Terminal | `uname -a` output shown |
| Workspace (grid) | Two workers side by side |
| Metrics | Request counts increased by the run's requests |
| Cost | Token totals increased by the run's turns |
| Logs | The run's request id found |
| Database | Read-only query returns rows |
| Disk | Used bytes reported and within 5% of `df` on the server |
| Proxies | Test proxy added and removed |
| Torrents | Public-domain test torrent added and removed |
| Converse and dictation | Chrome fake audio capture: fixed phrase transcribed |
| MCP | Test MCP server's tools listed |
| Computer use | Desktop screenshot through the computer endpoint |
| Vault and secrets | Secret stored, used by a worker, masked in the UI |
| Grants and standing approvals | A rule auto-approves a matching draft |
| Telegram | Test bot message arrives as a worker message; reply sent |
| Memories, journal, habits, CRM | One of each created and found by search |
| Settings | Theme, env key, default model changed and restored |
| Team presence | The invited account sees the owner online |
| Self-update | Release notes version equals `/health` version |

| Run | Where | Cost |
|---|---|---|
| Each PR touching `cloud/`, `site/`, billing, onboarding, `door.py` | Stripe test mode, throwaway server | One started server hour ($0.0137) and about $0.05 tokens |
| Hourly | Production synthetic customer (100% coupon, flagged, deleted after) | About $46/month (4.2) |
| Nightly | Provider test mode with subscription time travel (Stripe test clocks, or the merchant of record's sandbox simulations): dunning, cancellation, deletion; T-M1..T-M20, T-KILL-1, T-BUD-1 | Test mode |
| Weekly | Restore drill | One server-hour |
| Each iOS build | XCUITest against a test-mode server | CI minutes |

## 13. Build order, simplified

Phase A is the smallest thing that sells, provisions, bills and supports the
first paying customers. Everything else waits for evidence.

| Phase | Ships | Done when | Owner | Size |
|---|---|---|---|---|
| A0 | One-time setup (about 8 owner hours, once): written approval from Paddle for "managed hosting of open-source software" before relying on it, with the Stripe path (already modelled) as the fallback; the payment provider (O6) with products from `plans.json`; dedicated abuse@, support@ and privacy@ mailboxes, with abuse@ set as the Hetzner account's abuse contact; legal pages and data processing terms, Stripe Tax and Radar if Stripe stays, Clerk production, `amux.cloud` in Cloudflare, Hetzner quota raised, test inbox and bots, subscriber and trial list with decisions, launch price | Each item has evidence | amux-cloud; **[owner]** for quota, keys, price, O4 | S |
| A1 | Site: pricing, cloud page and homepage CTA; no trial; `plans.json` | T-SITE-1..3 on production | amux-homepage | S |
| A2 | Control plane: checkout-first accounts, `servers`, `provision_jobs`, hcloud and Cloudflare clients, redirect, emails, kill switches | T-PAY-1, T-ACC-1..4, T-RED-*, T-MAIL-*, T-KILL-1 | amux-cloud | M |
| A3 | Golden image, door token mode, key screen, templates | T-PROV-1, T-PROV-3, T-SSO-1, T-KEY-*, T-FIRST-1 | amux-cloud with amux | M |
| A4 | Lifecycle: dunning, cancel, export, delete; M1, M2, M5, M17, M19, M20 | T-DUN-1, T-CAN-*, T-DEL-1, T-PROV-4, T-M1, T-M2, T-M5, T-M17, T-M19, T-M20 | amux-cloud | M |
| A5 | Hourly synthetic customer, capability suite, weekly report, M11 | 7 days of hourly runs; first weekly report sent | amux-cloud | S |
| **Launch** | First paying customers | | | |
| B1 | Fleet agent: releases, backups, restore drill; M3, M4, M12 | T-OPS-1..4 | amux-cloud | M |
| B2 | Move to Cloud: bundle, importer, `amux cloud move`, the dashboard button, and every CTA in section 7.1 | T-MIG-1..5, T-SITE-1 | amux-cloud with amux; amux-homepage for site and README | M |
| B3 | Operator (one worker), MDAI nodes, M6 to M9, M13, M18, budgets | T-M6..T-M9, T-M13, T-BUD-1; 30 days without routine owner intervention | amux-cloud | M |
| C1 | Upgrade and downgrade, second server, teammates, help (M8, M10) | T-PLAN-1, T-PLAN-3, T-ADD-1, T-TEAM-1, T-HELP-1 | amux-cloud | M |
| C2 | iOS sign-in | T-IOS-1, T-IOS-2 | amux (iOS) | M |
| C3 | Content pipeline, referrals (M15, M16) | T-M15, T-M16; 90 days without routine owner intervention | Operator | S |
| Later, on evidence | Warm pool (when a cold wait under 3 minutes measurably hurts activation), US region (when EU customers churn citing latency), experiments engine at scale (when M14's traffic minimum is met weekly), more Operator workers (section 5 rule), Public Suffix List entry | Each gated on its trigger | | |

Cut from the earlier revision as unnecessary for the first customers: warm
pool, US region, region moves, and a multi-agent organisation on day one.

**First concrete step:** write `plans.json` and the price guard (M13), because
the site, Stripe, the control plane and the tests all read prices from it.

## 14. Iteration history

| Revision | Score | Scored by | Changes | Remaining gaps |
|---|---|---|---|---|
| 1 | 6.5 | Self | First draft | Journey and capability coverage, friction, owners |
| 2 | 9.1 | Self (inflated) | Redirect table, 26 tabs, no skips, checkout first, removals | Overturned by review |
| 2 | 6.6 | Independent reviewer | Found certificate limits, impossible downgrades, key handoff, unverified email, costs, App Store citation | |
| 3 | 7.65 | Independent reviewer | Fixed all of the above | Duplicate-certificate limit, PSL dependency, region moves, warm pool contradiction, Clerk verification |
| 4 | 6.3 | Independent reviewer, against the owner's revised brief | Rewritten for an owner-light business: objective, Operator, funnel and taxonomy, economics at scale, budgets, mechanisms M1 to M18, autonomy levels, kill switches, simplified phases; Cloudflare in front fixes the certificate issues | VAT and international fees missing; stopped servers bill; routine owner work hidden in abuse mail, disputes, support inbox and merges; mechanism placement undeclared; no volume model |
| 5 | 7.1 | Independent reviewer | Move-to-Cloud bundle, importer and CTAs (7.1); the dedicated monitoring worker and its source table (section 5); merchant of record (D11) and VAT-net economics with international and currency fees; servers never stopped (D12); real traffic volume model (4.7); Operator merge scope; where each mechanism runs, its status and test (10.0); escalation defaults; abuse isolation, data requests and disputes automated (M17, M19, M20); thin-traffic experiment rule; legal pages | |
| 6 | 7.7 | Independent reviewer | Bundle built from a per-table allowlist with schema migration in staging and a token scan (T-MIG-3); monitoring worker reads and recommends where the control plane acts; scoped Operator credentials (T-OP-1); App Store promotional text only; dedicated abuse@ mailbox; flat-traffic ceiling and the 1% planning row; Paddle approval in A0; section order and header fixed | Final weighted score below 9 by the last independent measure (7.1); see the report for what remains |
| 7 | not re-scored | | App Store key held by the control plane, Operator cap scaled with contribution and judged by profit, a 90-day test of whether organic traffic can grow, with a stop rule | See the final report |
