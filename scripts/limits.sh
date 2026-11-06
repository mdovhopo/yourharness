#!/bin/zsh
# Claude's usage limits right now, from the report the CLI sends alongside a tiny Haiku request
# (as HarnessCore's ClaudeUsageProbe does: no tools, no MCP, no settings, nothing saved).
# Prints e.g. "five_hour 23% resets 17:00 (1793991600)" and "seven_day 37% resets …".
# For pacing long autonomous work: pause before five_hour runs out, resume after it resets.
set -euo pipefail
cd /tmp
claude -p ping --output-format stream-json --verbose --model haiku --tools "" --strict-mcp-config \
    --setting-sources "" --no-session-persistence --system-prompt "Reply with one word." 2>/dev/null \
  | python3 -I -c '
import json, sys, time
for line in sys.stdin:
    try: o = json.loads(line)
    except ValueError: continue
    if o.get("type") != "rate_limit_event": continue
    for name, w in (o["rate_limit_info"].get("unifiedWindows") or {}).items():
        at = w.get("resetsAt", 0)
        pct = round(w.get("utilization", 0) * 100)
        print(name, f"{pct}%", "resets", time.strftime("%a %H:%M", time.localtime(at)), f"({at})", w.get("status", ""))
'
