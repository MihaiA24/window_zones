## Verification

- `/verify-window-zones` (`.agents/skills/verify-window-zones/`) launches, drives and checks this app. Run the checks that cover your change and trust their result. Prove new behavior on the real user path, and show the evidence.
- Never change an approved check's expected result yourself. When behavior changes on purpose, show me the old and new result and wait for my approval.
- When you change a feature on purpose, update its file in `.agents/skills/verify-window-zones/features/` in the same change.
- Ask before committing, pushing, opening a PR, or touching production, credentials or infrastructure.
