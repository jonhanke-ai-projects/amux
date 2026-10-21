#!/usr/bin/env python3
"""amux.io funnel report: how visitors reach the site's two goals.

Goal 1 is a GitHub star, goal 2 is a paid Cloud signup. The report reads
PostHog (project 378145) and the GitHub stargazers API and prints markdown:
the funnel for each goal against the prior window, then the same rates split
by traffic source and by landing page, so the daily AEO run (SCHED-149) can
see which pages and channels convert and which do not.

Events it reads (emitted by site/site.js and site/start/):
  $pageview                         visitors, landing page, referrer
  cta_click {goal, spot, href}      goal = github | cloud | pricing | walkthrough | ios | concierge
  install_copy                      copied the install command
  signup_form_start, signup_submit, checkout_redirect, signup_paid, server_ready

cta_click shipped 2026-10-19. Before that, GitHub clicks come from
$autocapture on github.com/mixpeek/amux links, so both are counted (per person,
so a click seen by both is one person).

  python3 scripts/site-funnel.py              # last 7 days vs the 7 before
  python3 scripts/site-funnel.py --days 14
  python3 scripts/site-funnel.py --json       # the same numbers, machine-readable
  python3 scripts/site-funnel.py --self-test  # source classifier only, offline

Needs POSTHOG_PERSONAL_API_KEY (read from the environment or ~/.amux/server.env).
"""
import argparse
import datetime as dt
import json
import os
import re
import subprocess
import sys
import urllib.request

PROJECT = 378145
PH = "https://us.posthog.com/api/projects/%d/query/" % PROJECT
REPO = "mixpeek/amux"

# Mirrors sourceOf() in site/site.js. Keep the two in step.
AI = re.compile(r"chatgpt|openai|perplexity|claude|gemini|copilot|phind|you\.com|grok|deepseek|mistral|kagi")
SEARCH = re.compile(r"(^|\.)(google|bing|duckduckgo|yahoo|yandex|baidu|ecosia|brave|startpage|naver)\.")
SOCIAL = re.compile(r"reddit|t\.co$|x\.com$|twitter|linkedin|ycombinator|youtube|facebook|threads|bsky|mastodon|discord")


def source_of(host, utm):
    h, s = (host or "").lower(), (utm or "").lower()
    if h == "$direct":
        h = ""
    if AI.search(s + " " + h):
        return "ai"
    if not h:
        return "campaign" if s else "direct"
    if re.search(r"(^|\.)amux\.io$", h):
        return "internal"
    if SEARCH.search(h):
        return "search"
    if h.endswith("github.com"):
        return "github"
    if SOCIAL.search(h):
        return "social"
    return "referral"


def self_test():
    cases = [
        (("chatgpt.com", ""), "ai"), (("", "chatgpt.com"), "ai"), (("www.perplexity.ai", ""), "ai"),
        (("gemini.google.com", ""), "ai"), (("www.google.com", ""), "search"), (("cn.bing.com", ""), "search"),
        (("$direct", ""), "direct"), (("", ""), "direct"), (("github.com", ""), "github"),
        (("t.co", ""), "social"), (("news.ycombinator.com", ""), "social"), (("amux.io", ""), "internal"),
        (("yetanotherorchestrator.app", ""), "referral"), (("", "newsletter"), "campaign"),
    ]
    bad = [(a, want, source_of(*a)) for a, want in cases if source_of(*a) != want]
    js = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "site", "site.js")).read()
    m = re.search(r"if \(/(chatgpt[^/]*)/\.test", js)
    if not m or m.group(1) != AI.pattern:
        bad.append(("site.js AI list", AI.pattern, m and m.group(1)))
    for a, want, got in bad:
        print("FAIL source_of%r = %s, want %s" % (a, got, want))
    print("%d/%d source checks pass" % (len(cases) + 1 - len(bad), len(cases) + 1))
    return 1 if bad else 0


def env_key(name):
    if os.environ.get(name):
        return os.environ[name]
    try:
        for line in open(os.path.expanduser("~/.amux/server.env")):
            if line.startswith(name + "="):
                return line.split("=", 1)[1].strip().strip("'\"")
    except OSError:
        pass
    return ""


def hogql(key, sql):
    body = json.dumps({"query": {"kind": "HogQLQuery", "query": sql}}).encode()
    req = urllib.request.Request(PH, body, {"Authorization": "Bearer " + key, "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=120) as r:
        d = json.load(r)
    return d.get("results") or []


GH_LINK = "(event = '$autocapture' and elements_chain_href like '%github.com/mixpeek/amux%')"
LIVE = "properties.$host = 'amux.io' and coalesce(toString(properties.test), '') != 'true'"


def people(key, start, end):
    """One row per visitor with a pageview in [start, end): first touch plus goal flags."""
    sql = f"""
select person_id,
  argMinIf(properties.$pathname, timestamp, event = '$pageview') as landing,
  argMinIf(properties.$referring_domain, timestamp, event = '$pageview') as ref,
  argMinIf(properties.utm_source, timestamp, event = '$pageview') as utm,
  argMinIf(properties.$device_type, timestamp, event = '$pageview') as device,
  countIf(event = '$pageview') as pv,
  countIf((event = 'cta_click' and properties.goal = 'github') or {GH_LINK}) > 0 as github,
  countIf(event = 'cta_click' and properties.goal = 'cloud') > 0 as cloud_click,
  countIf(event = '$pageview' and properties.$pathname in ('/start/', '/start')) > 0 as start_view,
  countIf(event = 'signup_form_start') > 0 as form_start,
  countIf(event = 'signup_submit') > 0 as submit,
  countIf(event = 'checkout_redirect') > 0 as checkout,
  countIf(event = 'signup_paid') > 0 as paid,
  countIf(event = 'server_ready') > 0 as ready,
  countIf(event = 'install_copy') > 0 as install_copy,
  countIf(event = '$pageview' and properties.$pathname like '/walkthrough%') > 0 as walkthrough,
  countIf(event = 'cta_click' and properties.goal = 'concierge') > 0 as concierge
from events
where timestamp >= toDateTime('{start:%Y-%m-%d %H:%M:%S}') and timestamp < toDateTime('{end:%Y-%m-%d %H:%M:%S}')
  and {LIVE}
group by person_id
having pv > 0
limit 100000"""
    cols = ["person", "landing", "ref", "utm", "device", "pv", "github", "cloud_click", "start_view",
            "form_start", "submit", "checkout", "paid", "ready", "install_copy", "walkthrough", "concierge"]
    rows = [dict(zip(cols, r)) for r in hogql(key, sql)]
    for r in rows:
        r["source"] = source_of(r["ref"], r["utm"])
        for c in cols[6:]:
            r[c] = bool(r[c])
    return rows


def event_counts(key, start, end):
    sql = f"""
select event, count() from events
where timestamp >= toDateTime('{start:%Y-%m-%d %H:%M:%S}') and timestamp < toDateTime('{end:%Y-%m-%d %H:%M:%S}')
  and {LIVE} and event in ('cta_click', 'install_copy', 'signup_form_start', 'signup_submit', 'checkout_redirect', 'signup_paid', 'server_ready')
group by event"""
    return dict(hogql(key, sql))


def cta_spots(key, start, end):
    sql = f"""
select properties.goal, properties.$pathname, properties.spot, count(distinct person_id) from events
where event = 'cta_click' and timestamp >= toDateTime('{start:%Y-%m-%d %H:%M:%S}')
  and timestamp < toDateTime('{end:%Y-%m-%d %H:%M:%S}') and {LIVE}
group by 1, 2, 3 order by 4 desc limit 40"""
    return hogql(key, sql)


def new_stars(start, end, prior_start):
    """New stargazers per window from the GitHub API. None when it cannot be read."""
    hdr = {"Accept": "application/vnd.github.star+json", "User-Agent": "amux-site-funnel"}
    # The stargazer list needs auth (anonymous calls 401). Use a token from the
    # environment, else the gh CLI's.
    tok = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if not tok:
        try:
            tok = subprocess.run(["gh", "auth", "token"], capture_output=True, text=True, timeout=15).stdout.strip()
        except (OSError, subprocess.SubprocessError):
            tok = ""
    if tok:
        hdr["Authorization"] = "Bearer " + tok
    try:
        with urllib.request.urlopen(urllib.request.Request("https://api.github.com/repos/" + REPO, headers=hdr), timeout=30) as r:
            total = json.load(r)["stargazers_count"]
        page, dates = (total + 99) // 100, []
        while page >= 1:
            url = "https://api.github.com/repos/%s/stargazers?per_page=100&page=%d" % (REPO, page)
            with urllib.request.urlopen(urllib.request.Request(url, headers=hdr), timeout=30) as r:
                got = [dt.datetime.fromisoformat(s["starred_at"].replace("Z", "+00:00")).replace(tzinfo=None) for s in json.load(r)]
            dates += got
            if not got or min(got) < prior_start:
                break
            page -= 1
    except Exception as e:  # rate limit or network: say so rather than print 0
        return {"measured": False, "why": str(e)[:120]}
    return {"measured": True, "total": total,
            "now": sum(start <= d < end for d in dates), "prior": sum(prior_start <= d < start for d in dates)}


STEPS = [
    ("github", "Clicked a GitHub link"),
    ("start_view", "Viewed /start/ (Cloud signup page)"),
    ("form_start", "Started the signup form"),
    ("submit", "Submitted the form"),
    ("checkout", "Reached Stripe checkout"),
    ("paid", "Paid (reached /start/welcome/)"),
    ("ready", "Server ready"),
    ("install_copy", "Copied the install command"),
    ("walkthrough", "Viewed the walkthrough"),
    ("concierge", "Clicked Schedule a meeting (concierge)"),
]


def pct(n, d):
    return "%.1f%%" % (100.0 * n / d) if d else "n/a"


def delta(a, b):
    if not b:
        return "new" if a else "0"
    return "%+.0f%%" % (100.0 * (a - b) / b)


def summarize(rows):
    out = {"visitors": len(rows)}
    for k, _ in STEPS:
        out[k] = sum(r[k] for r in rows)
    return out


def split(rows, key):
    groups = {}
    for r in rows:
        groups.setdefault(r[key] or "(none)", []).append(r)
    return {k: summarize(v) for k, v in groups.items()}


def render(days, start, end, now, prior, by_source, by_landing, events, spots, stars):
    L = []
    L.append("# amux.io funnel: %s to %s (%d days, vs the %d before)" % (start.date(), (end - dt.timedelta(seconds=1)).date(), days, days))
    L.append("")
    L.append("Visitors are people with a pageview on amux.io (localhost and test checkouts excluded): **%d** (prior %d, %s)." % (now["visitors"], prior["visitors"], delta(now["visitors"], prior["visitors"])))
    L.append("")
    L.append("| step | people | % of visitors | prior | change |")
    L.append("|---|---|---|---|---|")
    for k, label in STEPS:
        L.append("| %s | %d | %s | %d | %s |" % (label, now[k], pct(now[k], now["visitors"]), prior[k], delta(now[k], prior[k])))
    if stars.get("measured"):
        L.append("| New GitHub stars (GitHub API, not per visitor) | %d | | %d | %s |" % (stars["now"], stars["prior"], delta(stars["now"], stars["prior"])))
    else:
        L.append("| New GitHub stars | not measured: %s | | | |" % stars.get("why"))
    L.append("")
    n_cta = events.get("cta_click", 0)
    L.append("Instrumented events this window: %s." % (", ".join("%s %d" % kv for kv in sorted(events.items())) or "none"))
    if not n_cta:
        L.append("cta_click: 0 in this window, so GitHub clicks come from $autocapture only and Cloud intent from /start/ views.")
    L.append("")
    L.append("## By traffic source (first touch in the window)")
    L.append("")
    L.append("| source | visitors | GitHub click | /start/ view | paid | install copy |")
    L.append("|---|---|---|---|---|---|")
    for k, s in sorted(by_source.items(), key=lambda kv: -kv[1]["visitors"]):
        L.append("| %s | %d | %s | %s | %d | %s |" % (k, s["visitors"], pct(s["github"], s["visitors"]), pct(s["start_view"], s["visitors"]), s["paid"], pct(s["install_copy"], s["visitors"])))
    L.append("")
    L.append("## Landing pages (top 20 by visitors)")
    L.append("")
    L.append("| landing page | visitors | GitHub click | /start/ view | paid |")
    L.append("|---|---|---|---|---|")
    top = sorted(by_landing.items(), key=lambda kv: -kv[1]["visitors"])
    for k, s in top[:20]:
        L.append("| %s | %d | %s | %s | %d |" % (k, s["visitors"], pct(s["github"], s["visitors"]), pct(s["start_view"], s["visitors"]), s["paid"]))
    gaps = [(k, s) for k, s in top if s["visitors"] >= 10 and not (s["github"] or s["start_view"] or s["install_copy"] or s["concierge"])]
    L.append("")
    L.append("## Gaps: landing pages with 10+ visitors and no goal action")
    L.append("")
    L.append("\n".join("- %s (%d visitors)" % (k, s["visitors"]) for k, s in gaps) or "- none")
    L.append("")
    L.append("## Where goal clicks happen (cta_click, distinct people)")
    L.append("")
    if spots:
        L.append("| goal | page | spot | people |")
        L.append("|---|---|---|---|")
        L.extend("| %s | %s | %s | %d |" % tuple(r) for r in spots)
    else:
        L.append("No cta_click events in this window.")
    return "\n".join(L)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--days", type=int, default=7)
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    key = env_key("POSTHOG_PERSONAL_API_KEY")
    if not key:
        print("POSTHOG_PERSONAL_API_KEY is not set (environment or ~/.amux/server.env)", file=sys.stderr)
        return 2
    end = dt.datetime.utcnow().replace(minute=0, second=0, microsecond=0)
    start = end - dt.timedelta(days=a.days)
    prior_start = start - dt.timedelta(days=a.days)
    rows, prow = people(key, start, end), people(key, prior_start, start)
    now, prior = summarize(rows), summarize(prow)
    by_source, by_landing = split(rows, "source"), split(rows, "landing")
    events, spots = event_counts(key, start, end), cta_spots(key, start, end)
    stars = new_stars(start, end, prior_start)
    if a.json:
        print(json.dumps({"window": {"start": start.isoformat(), "end": end.isoformat(), "days": a.days},
                          "now": now, "prior": prior, "by_source": by_source, "by_landing": by_landing,
                          "events": events, "cta_spots": spots, "stars": stars}, indent=1, default=str))
    else:
        print(render(a.days, start, end, now, prior, by_source, by_landing, events, spots, stars))
    return 0


if __name__ == "__main__":
    sys.exit(main())
