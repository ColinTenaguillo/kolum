//! The demo board's content. Plain tables so a new ticket is one line, and
//! descriptions in Markdown so they read as what someone typed.

use kolum_domain::{IssueKind, Priority};

/// The priorities of Jira's default scheme, the most urgent first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Prio {
    Highest,
    High,
    Medium,
    Low,
    Lowest,
}

impl Prio {
    pub const ALL: [Prio; 5] = [Prio::Highest, Prio::High, Prio::Medium, Prio::Low, Prio::Lowest];

    pub fn priority(self) -> Priority {
        Priority { rank: self as u32, name: format!("{self:?}") }
    }
}

pub const PROJECT_KEY: &str = "ACME";
pub const PROJECT_NAME: &str = "Acme Web";

/// (id, display name). The first is the signed-in user.
pub const USERS: &[(&str, &str)] = &[
    ("u-colin", "Colin Tenaguillo"),
    ("u-ana", "Ana Silva"),
    ("u-bruno", "Bruno Costa"),
    ("u-chloe", "Chloé Martin"),
    ("u-dev", "Dev Patel"),
    ("u-emma", "Emma Johansson"),
];

/// (id, name, category: 0 todo, 1 doing, 2 done).
pub const STATUSES: &[(&str, &str, u8)] = &[
    ("backlog", "Backlog", 0),
    ("todo", "To Do", 0),
    ("progress", "In Progress", 1),
    ("blocked", "Blocked", 1),
    ("review", "In Review", 1),
    ("done", "Done", 2),
];

/// (name, statuses, WIP limit). "In Progress" holds two statuses, so a
/// card moved into it has two transitions to choose from.
pub const COLUMNS: &[(&str, &[&str], Option<u32>)] = &[
    ("Backlog", &["backlog"], None),
    ("To Do", &["todo"], None),
    ("In Progress", &["progress", "blocked"], Some(4)),
    ("In Review", &["review"], Some(3)),
    ("Done", &["done"], None),
];

pub struct Row {
    pub n: u32,
    pub kind: IssueKind,
    pub status: &'static str,
    pub priority: Prio,
    /// Index into USERS.
    pub assignee: Option<usize>,
    pub reporter: usize,
    pub labels: &'static [&'static str],
    /// The epic's number.
    pub epic: Option<u32>,
    pub points: Option<f64>,
    pub title: &'static str,
    pub description: &'static str,
}

use IssueKind::{Bug, Epic, Story, Task};
use Prio::{High, Highest, Low, Lowest, Medium};

macro_rules! rows {
    ($( $n:literal $kind:ident $status:literal $prio:ident $assignee:expr, $reporter:literal,
        [$($label:literal),*] $epic:expr, $points:expr, $title:literal $desc:expr; )*) => {
        &[$(Row {
            n: $n, kind: $kind, status: $status, priority: $prio, assignee: $assignee,
            reporter: $reporter, labels: &[$($label),*], epic: $epic, points: $points,
            title: $title, description: $desc,
        }),*]
    };
}

const SSO: &str = "\
## Goal

Let customers sign in with their company identity provider instead of a password.

- SAML 2.0 and OIDC
- Just-in-time provisioning of accounts
- Enforced SSO per organisation

> [!NOTE]
> Okta and Azure AD cover 80% of the requests from sales.";

const LOGIN_TIMEOUT: &str = "\
After the SSO redirect, the callback sometimes takes more than **30 s** and the \
login page shows *Session expired*.

### Steps to reproduce

1. Configure an Okta tenant with MFA
2. Sign in from a cold browser
3. Wait on the push notification for ~25 s

### Logs

```log
2026-10-12T09:14:03Z WARN auth::callback state=7f3a.. elapsed=31.2s
2026-10-12T09:14:03Z ERROR auth::session expired before exchange
```

The state cookie has a 30 s TTL, see `auth/session.rs`. Ping [@Ana Silva](mention:u-ana) \
who wrote the exchange.";

const RATE_LIMIT: &str = "\
The public API has no rate limiting. A single client hammered `/v1/search` last week.

| Plan | Requests / min | Burst |
|------|----------------|-------|
| Free | 60 | 10 |
| Pro | 600 | 100 |
| Enterprise | 6000 | 1000 |

> [!WARNING]
> Return `429` with a `Retry-After` header, never drop silently.";

const OFFLINE: &str = "\
## Why

Field technicians lose signal in basements and still need their work orders.

## Scope

- Read-only cache of the next 48 h of work orders
- Queue of status changes, replayed on reconnect
- ~~Full offline editing~~ (later)";

const CSV_EXPORT: &str = "\
As an accountant I want to export invoices as CSV so that I can import them in my \
ledger.

**Acceptance criteria**

- [ ] One row per invoice line
- [ ] UTF-8 with BOM for Excel
- [ ] Dates in ISO 8601

```sql
SELECT number, issued_at, total_cents FROM invoices WHERE org_id = $1;
```";

const DARK_MODE: &str = "\
The settings page ignores `prefers-color-scheme`. Follow the system by default and \
keep the toggle as an override.

See the [design spec](https://design.acme.invalid/dark-mode).";

const FLAKY: &str = "\
`checkout_flow_test` fails about one run in ten on CI.

```rust
#[tokio::test]
async fn checkout_flow_test() {
    let cart = Cart::with_items(3);
    // Races with the price refresh job.
    assert_eq!(cart.total().await, 4200);
}
```

> [!CAUTION]
> Do not just add a retry: the race is real in production too.";

const WEBHOOKS: &str = "\
Customers want to be told when an invoice is paid.

1. `invoice.paid`
2. `invoice.failed`
3. `subscription.cancelled`

Sign every payload with HMAC-SHA256 and document the verification.";

pub const ROWS: &[Row] = rows! {
    1 Epic "progress" High None, 0, ["auth"] None, None, "Single sign-on" SSO;
    2 Epic "todo" Medium None, 3, ["mobile"] None, None, "Offline mode for field technicians" OFFLINE;
    3 Epic "progress" High None, 2, ["billing"] None, None, "Billing revamp" "Move invoicing off the legacy monolith.";
    4 Epic "backlog" Low None, 4, ["growth"] None, None, "Mobile onboarding" "A first-run flow that gets a new technician to their first work order.";
    5 Bug "progress" Highest Some(0), 1, ["auth", "backend"] Some(1), Some(3.0), "Login times out after SSO redirect" LOGIN_TIMEOUT;
    6 Story "review" High Some(1), 0, ["auth"] Some(1), Some(5.0), "SAML assertion consumer endpoint" "Accept signed assertions on `/sso/saml/acs` and map attributes to the user.";
    7 Story "todo" Medium Some(1), 0, ["auth"] Some(1), Some(8.0), "Just-in-time provisioning of SSO users" "Create the account on first sign-in when the organisation allows it.";
    8 Task "done" Medium Some(2), 1, ["auth", "infra"] Some(1), Some(2.0), "Rotate the OIDC client secrets" "Move them to the vault and rotate every 90 days.";
    9 Story "backlog" Low None, 0, ["auth"] Some(1), Some(5.0), "Enforce SSO per organisation" "An admin switch that disables password sign-in for the whole organisation.";
    10 Bug "blocked" High Some(4), 2, ["backend", "api"] None, Some(3.0), "Search API returns duplicates on page boundaries" "Offset pagination over a non-unique sort. Blocked on the index migration.";
    11 Story "progress" High Some(2), 4, ["api", "backend"] None, Some(5.0), "Rate limiting for the public API" RATE_LIMIT;
    12 Story "todo" Medium Some(3), 2, ["billing"] Some(3), Some(3.0), "Export invoices as CSV" CSV_EXPORT;
    13 Story "progress" Medium Some(0), 2, ["billing", "backend"] Some(3), Some(8.0), "Webhooks for invoice events" WEBHOOKS;
    14 Task "review" Medium Some(3), 2, ["billing"] Some(3), Some(2.0), "Migrate invoice PDFs to the new renderer" "Pixel-compare 200 sample invoices before switching.";
    15 Bug "done" High Some(2), 3, ["billing"] Some(3), Some(1.0), "VAT rounded twice on multi-currency invoices" "Round once, on the total, in the invoice currency.";
    16 Story "backlog" Medium None, 2, ["billing"] Some(3), Some(13.0), "Usage-based pricing" "Bill API calls per thousand, with a monthly cap.";
    17 Story "todo" Medium Some(5), 3, ["mobile"] Some(2), Some(8.0), "Cache the next 48 h of work orders" "SQLite on device, refreshed on every foreground.";
    18 Story "backlog" Medium None, 3, ["mobile"] Some(2), Some(8.0), "Replay queued status changes on reconnect" "Last write wins, with a conflict banner when the server disagrees.";
    19 Bug "todo" High Some(5), 1, ["mobile", "frontend"] None, Some(2.0), "Camera permission prompt shown twice on Android 15" "Android 15 re-asks after the app is restored from the background.";
    20 Story "progress" Low Some(3), 0, ["frontend", "ux"] None, Some(2.0), "Dark mode follows the system setting" DARK_MODE;
    21 Bug "progress" High Some(4), 4, ["ci", "backend"] None, Some(3.0), "Flaky checkout_flow_test on CI" FLAKY;
    22 Task "done" Low Some(4), 4, ["ci"] None, Some(1.0), "Cache cargo registry in CI" "Saves about four minutes per run.";
    23 Task "done" Medium Some(0), 0, ["infra"] None, Some(2.0), "Upgrade Postgres to 17" "Rehearsed on staging, 4 minutes of read-only.";
    24 Story "review" Medium Some(5), 3, ["frontend"] None, Some(3.0), "Keyboard shortcuts on the dispatch board" "`j`/`k` to move, `a` to assign, `?` for help.";
    25 Bug "todo" Medium Some(2), 5, ["frontend"] None, Some(1.0), "Date picker shows the wrong week start in fr-FR" "Weeks start on Monday in France.";
    26 Task "backlog" Lowest None, 5, ["docs"] None, None, "Document the webhook signature check" "Examples in Python, Node and Rust.";
    27 Story "backlog" Low None, 1, ["growth"] Some(4), Some(5.0), "Welcome checklist on first launch" "Three steps: profile, first job, notifications.";
    28 Story "backlog" Low None, 1, ["growth", "mobile"] Some(4), Some(3.0), "Sample work order for new technicians" "A fake job they can complete to learn the flow.";
    29 Bug "review" Highest Some(0), 3, ["backend", "billing"] Some(3), Some(2.0), "Stripe retries create duplicate payments" "Use the idempotency key from the invoice id.";
    30 Task "progress" Medium Some(1), 1, ["infra"] None, Some(3.0), "Move staging to the new cluster" "Terraform is ready, DNS cut-over pending.";
    31 Story "todo" Low Some(4), 0, ["api"] None, Some(2.0), "Return ETag headers on GET endpoints" "So clients can revalidate cheaply.";
    32 Bug "done" Medium Some(3), 1, ["frontend"] None, Some(1.0), "Tooltip hidden behind the map on Safari" "A z-index fight with the map's canvas.";
    33 Task "todo" Medium Some(0), 4, ["security"] None, Some(1.0), "Review the npm audit findings" "Twelve moderate, none reachable from user input so far.";
    34 Story "backlog" Medium None, 2, ["api"] None, Some(5.0), "GraphQL endpoint for the partner portal" "Read-only to start, behind a feature flag.";
    35 Bug "blocked" Medium Some(5), 5, ["mobile"] Some(2), Some(2.0), "iOS widget shows stale jobs" "Waiting on the background refresh entitlement from Apple.";
    36 Task "done" Low Some(1), 0, ["docs", "auth"] Some(1), Some(1.0), "Write the SSO setup guide for admins" "Okta, Azure AD and Google Workspace, with screenshots.";
    37 Story "todo" High Some(3), 2, ["billing", "frontend"] Some(3), Some(5.0), "Invoice history page" "Paginated, filterable by status and date.";
    38 Bug "backlog" Low None, 4, ["frontend"] None, None, "Long customer names overflow the card" "Truncate with an ellipsis and show the full name on hover.";
    39 Task "review" Low Some(2), 2, ["infra", "ci"] None, Some(2.0), "Nightly backup restore test" "Restore last night's dump to a scratch instance and run smoke tests.";
    40 Story "done" Medium Some(5), 3, ["mobile"] Some(4), Some(3.0), "Push notifications for new jobs" "FCM and APNs, with a quiet-hours setting.";
    41 Bug "todo" Lowest Some(1), 5, ["ux"] None, None, "Typo in the password reset e-mail" "\"recieve\" -> \"receive\".";
    42 Story "progress" Medium Some(5), 3, ["mobile", "ux"] Some(2), Some(5.0), "Offline indicator in the app bar" "Grey bar with the time of the last sync.";
    43 Task "backlog" Low None, 0, ["security", "infra"] None, Some(3.0), "Enable audit logging for admin actions" "Who changed what, kept 1 year.";
    44 Bug "done" High Some(0), 1, ["api"] None, Some(2.0), "500 on /v1/jobs when the filter is empty" "An empty `IN ()` clause.";
};

/// (issue number, author, Markdown).
pub const COMMENTS: &[(u32, usize, &str)] = &[
    (5, 1, "I can reproduce with MFA on. Without MFA it's fine."),
    (5, 0, "Raising the TTL to 120 s fixes it locally. Is there a reason it was 30?"),
    (5, 1, "Only caution. 120 s is fine, the state is single-use anyway."),
    (6, 0, "Can we log the assertion's `NotOnOrAfter` when it's rejected?"),
    (10, 4, "> [!NOTE]\n> Blocked until ACME-34's index lands."),
    (
        11,
        2,
        "Going with a token bucket in Redis:\n\n```lua\nlocal tokens = redis.call('GET', KEYS[1])\n```",
    ),
    (21, 4, "Reproduced 3/30 locally with `--test-threads=8`."),
    (21, 0, "The price refresh job takes the cart lock in a different order. Swapping it."),
    (29, 3, "Stripe's docs say the key must be stable across retries: the invoice id is."),
    (13, 2, "Which events do we need for the first release? Paid and failed only?"),
    (13, 0, "Paid, failed and cancelled. **Cancelled** is the one sales keeps asking for."),
];
