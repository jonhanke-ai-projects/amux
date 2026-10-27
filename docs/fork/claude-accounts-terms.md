# Running several Claude accounts: where we stand on the terms

Analysis from 2026-10-27, done by Claude (Opus 5.5). It is not legal advice. Re-check it if the terms change.

## Setup

- Two Claude Max accounts (20x). Both are Jon's, and both are billed to Jon's card.
- Jon is the only person using either one. The second account may later go to an assistant for their own work.
- Plan: split projects across the two accounts (fork feature, issue #1 / PR #2), and spend leftover quota near each account's weekly reset.

## Conclusion: fine as set up

| Question | Answer | Basis |
|---|---|---|
| Using a Max 20x account up to its limits | Fine. That is what the plan is for. | Usage limits are the allowance you pay for |
| Paying for both accounts on one card | Fine | Consumer Terms §6 only requires accurate billing information |
| One person using two accounts | No sharing problem | §2: may not share login or "make your Account available to anyone else"; nobody else uses them |
| One person holding more than one account | Not addressed. Low risk, but not explicitly approved | The Terms are silent. Claude Code docs say limits "assume ordinary, individual usage"; that is context, not a ban. Anthropic's enforcement has targeted sharing, reselling and third-party tools |
| amux driving Claude Code | Fine | Subscription login is allowed for Claude Code itself, and amux runs the real, unmodified `claude` program |

## Watch for

1. **If the second account goes to an assistant**, it becomes their account. Stop routing Jon's projects or overflow onto it (`amux claude-accounts assign --group <g> --default` or onto another account of Jon's). The clean setup then is a Claude Team plan, or a separate account in their name that Jon pays for.
2. **Leave these upstream amux features unused:** the rotation script's scripted email-link sign-in, the keychain token reader for `/api/usage`, and the dashboard's saved sign-in token. The Claude Code docs bar tools that "collect, store, or intermediate" Claude.ai credentials. The fork's own parts stay clear of this:
   - Sign-in goes through `claude auth login`.
   - The usage tap only reads what Claude Code passes to the status line.
3. **For certainty**, send a one-line written question to Anthropic support ("I hold two Max accounts, both used only by me — is that OK?") and keep the reply.

## Sources

- [Anthropic Consumer Terms of Service](https://www.anthropic.com/legal/consumer-terms) (effective Oct 8, 2025)
- [Claude Code: Legal and compliance](https://code.claude.com/docs/en/legal-and-compliance)
- [Using Claude Code with your Pro or Max plan](https://support.claude.com/en/articles/11145838-using-claude-code-with-your-pro-or-max-plan)
- [TechCrunch, 2025-07-28: rate limits to curb account sharing and reselling](https://techcrunch.com/2025/07/28/anthropic-unveils-new-rate-limits-to-curb-claude-code-power-users/)
