# Fills a data folder with a year of Claude Code history for the usage view: session files
# under claude-config/projects (point CLAUDE_CONFIG_DIR at it), as Claude Code writes them,
# with replies split into a line per content block, sub-agents' files, forks that copy their
# parent's history, synthetic notices, and the models of each era. Plus a state.json with a
# limits reading. Dates are relative to today, so the newest day is always today.
#   python3 generate.py <data folder> [scale]    (scale 1 is about 2,000 sessions, 250 MB)
import datetime, json, os, random, sys, uuid

out = sys.argv[1]
scale = float(sys.argv[2]) if len(sys.argv) > 2 else 1.0
rng = random.Random(11)
config = os.path.join(out, "claude-config", "projects")
now = datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0)
words = ("parser migration index thread session memory window sidebar transcript render layout switch commit branch "
         "release build test flaky timezone cache budget token query search worktree review merge stack diagram").split()


def sentence(n):
    return " ".join(rng.choice(words) for _ in range(n)).capitalize() + "."


def uid():
    return str(uuid.UUID(int=rng.getrandbits(128), version=4))


def stamp(moment):
    return moment.astimezone(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.") + f"{moment.microsecond // 1000:03d}Z"


def era(days_ago):
    """(main model, everyday model, sub-agent model, rare model) as they changed over the year."""
    if days_ago > 200:
        return "claude-opus-4-5-20251101", "claude-sonnet-4-5-20250929", "claude-haiku-4-5-20251001", None
    if days_ago > 90:
        return "claude-opus-5", "claude-sonnet-5", "claude-haiku-4-5-20251001", "claude-opus-4-8"
    return "claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-fable-5-1"


def reply_lines(session, cwd, model, moment, context, sidechain=False):
    """One API response as Claude Code writes it: a line per content block, the last with the
    final output count."""
    message, request = "msg_01" + uid().replace("-", "")[:22], "req_01" + uid().replace("-", "")[:22]
    write = rng.randint(300, 9_000)
    output = rng.randint(80, 3_500)
    usage = {"input_tokens": rng.randint(2, 60), "cache_creation_input_tokens": write, "cache_read_input_tokens": context,
             "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": write},
             "output_tokens": output, "service_tier": "standard"}
    blocks = [{"type": "thinking", "thinking": "", "signature": uid()}]
    if rng.random() < 0.7:
        blocks.append({"type": "tool_use", "id": "toolu_" + uid()[:20], "name": rng.choice(["Bash", "Read", "Edit", "Grep"]),
                       "input": {"command": f"swift test --filter {rng.choice(words)}", "description": sentence(4)}})
    blocks.append({"type": "text", "text": " ".join(sentence(rng.randint(6, 20)) for _ in range(rng.randint(1, 6)))})
    lines = []
    for index, block in enumerate(blocks):
        partial = dict(usage, output_tokens=output if index == len(blocks) - 1 else max(1, output * index // len(blocks)))
        lines.append({"parentUuid": uid(), "isSidechain": sidechain, "userType": "external", "cwd": cwd, "sessionId": session,
                      "version": "2.1.300", "gitBranch": "main",
                      "message": {"id": message, "type": "message", "role": "assistant", "model": model, "content": [block],
                                  "stop_reason": None, "stop_sequence": None, "usage": partial},
                      "requestId": request, "type": "assistant", "uuid": uid(), "timestamp": stamp(moment)})
        moment += datetime.timedelta(milliseconds=rng.randint(200, 4_000))
    return lines, write


def user_line(session, cwd, moment):
    result = "\n".join(sentence(12) for _ in range(rng.randint(3, 40)))
    return {"parentUuid": uid(), "isSidechain": False, "userType": "external", "cwd": cwd, "sessionId": session,
            "version": "2.1.300", "type": "user", "uuid": uid(), "timestamp": stamp(moment),
            "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_" + uid()[:20], "content": result}]}}


def write(path, lines):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as file:
        for line in lines:
            file.write(json.dumps(line, separators=(",", ":")) + "\n")
    mtime = datetime.datetime.fromisoformat(lines[-1]["timestamp"].replace("Z", "+00:00")).timestamp()
    os.utime(path, (mtime, mtime))


folders = ["-Users-me-dev-acme-api", "-Users-me-dev-acme-web", "-Users-me-dev-tools", "-Users-me-notes"]
sessions = files = responses = 0
for days_ago in range(364, -1, -1):
    day = (now - datetime.timedelta(days=days_ago)).replace(hour=8, minute=0, second=0)
    weekend = day.weekday() >= 5
    if rng.random() < (0.55 if weekend else 0.08):
        continue  # a day off
    main, everyday, helper, rare = era(days_ago)
    count = max(1, int(rng.randint(1, 4 if weekend else 9) * scale))
    for _ in range(count):
        session, folder = uid(), rng.choice(folders)
        cwd = "/" + folder.strip("-").replace("-", "/")
        model = rng.choices([main, everyday, rare or main], weights=[6, 3, 1 if rare else 0])[0]
        moment = day + datetime.timedelta(minutes=rng.randint(0, 13 * 60))
        if days_ago == 0:
            moment = min(moment, now - datetime.timedelta(hours=1))
        lines = [{"parentUuid": None, "isSidechain": False, "userType": "external", "cwd": cwd, "sessionId": session,
                  "version": "2.1.300", "type": "user", "uuid": uid(), "timestamp": stamp(moment),
                  "message": {"role": "user", "content": sentence(rng.randint(6, 30))}}]
        context = rng.randint(15_000, 30_000)
        for _ in range(rng.randint(8, 90)):
            moment += datetime.timedelta(seconds=rng.randint(4, 90))
            reply, written = reply_lines(session, cwd, model, moment, context)
            lines += reply
            lines.append(user_line(session, cwd, moment))
            context = min(context + written, 400_000)
            responses += 1
        if rng.random() < 0.05:
            lines.append({"parentUuid": uid(), "isSidechain": False, "type": "assistant", "uuid": uid(), "timestamp": stamp(moment),
                          "isApiErrorMessage": True, "message": {"id": "msg_" + uid(), "model": "<synthetic>", "role": "assistant",
                          "content": [{"type": "text", "text": "API Error: overloaded"}], "usage": {"input_tokens": 0, "output_tokens": 0}}})
        write(os.path.join(config, folder, f"{session}.jsonl"), lines)
        sessions += 1
        files += 1
        # Sub-agents, in files of their own beside the session.
        for agent in range(rng.choice([0, 0, 1, 2, 3])):
            agent_lines, agent_moment = [], moment - datetime.timedelta(minutes=rng.randint(1, 30))
            agent_context = rng.randint(8_000, 20_000)
            for _ in range(rng.randint(4, 30)):
                agent_moment += datetime.timedelta(seconds=rng.randint(2, 20))
                reply, written = reply_lines(session, cwd, helper if rng.random() < 0.8 else everyday, agent_moment, agent_context, sidechain=True)
                agent_lines += reply
                agent_context += written
                responses += 1
            write(os.path.join(config, folder, session, "subagents", f"agent-{uid()[:8]}.jsonl"), agent_lines)
            files += 1
        # A fork: a new session starting from a copy of this one's history.
        if rng.random() < 0.04:
            fork = uid()
            copied = [dict(line, sessionId=fork) for line in lines[: len(lines) // 2]]
            write(os.path.join(config, folder, f"{fork}.jsonl"), copied)
            files += 1

home = "AAAAAAAA-0000-4000-8000-0000000000A1"
state = {
    "projects": [{"id": home, "name": "Home", "path": "{{ROOT}}/home", "permissionMode": "auto"}],
    "homeProjectID": home,
    "threads": [{"id": "BBBBBBBB-0000-4000-8000-0000000000B1", "projectID": home, "title": "Usage check",
                 "createdAt": stamp(now - datetime.timedelta(hours=2)), "updatedAt": stamp(now - datetime.timedelta(hours=1)), "model": "haiku"}],
    "selectedThreadID": "BBBBBBBB-0000-4000-8000-0000000000B1",
    "defaults": {"model": "haiku"},
    "titleGeneration": {"provider": "claude", "model": "haiku"},
    "rateLimit": {"status": "allowed", "windows": [
        {"name": "five_hour", "utilization": 0.42, "resetsAt": (now + datetime.timedelta(hours=2, minutes=14)).strftime("%Y-%m-%dT%H:%M:%SZ")},
        {"name": "seven_day", "utilization": 0.67, "resetsAt": (now + datetime.timedelta(days=3, hours=5)).strftime("%Y-%m-%dT%H:%M:%SZ")},
    ]},
    "rateLimitUpdatedAt": (now - datetime.timedelta(minutes=6)).strftime("%Y-%m-%dT%H:%M:%SZ"),
}
with open(os.path.join(out, "state.json"), "w") as file:
    json.dump(state, file, indent=1)
print(f"{sessions} sessions, {files} files, {responses} responses in {config}")
