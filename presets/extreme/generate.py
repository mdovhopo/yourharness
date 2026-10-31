# Fills a data folder with threads at the far end of real use, in the 0.0.6 JSON format:
# millions of tokens of history across many compactions, turns of hundreds of tool calls with
# reasoning between them, some huge results. For YH_PERF_PROBE (scripts/perf-run.sh).
#   python3 generate.py <data folder> [scale]
# Scale 1: 20 huge threads, 4 of 30 compactions and about 32,000 rows, 16 of half that, about
# 2 GB in all (up to 40M tokens a thread by size, past anything real on purpose), plus 100
# ordinary ones to fill the sidebar.
import json, os, random, sys, uuid

out = sys.argv[1]
scale = float(sys.argv[2]) if len(sys.argv) > 2 else 1
rng = random.Random(11)
words = ("parser migration index thread session memory window sidebar transcript render layout switch commit branch "
         "release build test flaky timezone cache budget token query search worktree review merge stack diagram").split()

def sentence(n):
    return " ".join(rng.choice(words) for _ in range(n)).capitalize() + "."

def uid():
    return str(uuid.UUID(int=rng.getrandbits(128), version=4)).upper()

def stamp(day, minute):
    return f"2026-{9 + day // 28:02d}-{day % 28 + 1:02d}T{minute // 60 % 24:02d}:{minute % 60:02d}:00Z"

def answer():
    parts = [sentence(25) for _ in range(rng.randint(1, 3))]
    if rng.random() < 0.4:
        parts.append("```swift\n" + "\n".join(f"let {rng.choice(words)}{i} = {i}" for i in range(rng.randint(5, 30))) + "\n```")
    if rng.random() < 0.3:
        parts.append("| Part | Before | After |\n|---|---|---|\n" + "\n".join(f"| {rng.choice(words)} | {i} ms | {i // 2} ms |" for i in range(6)))
    if rng.random() < 0.5:
        parts.append("\n".join(f"- `{rng.choice(words)}.swift`: {sentence(10)}" for _ in range(4)))
    return "\n\n".join(parts)

def tool(day, minute):
    name = rng.choice(["Bash", "Read", "Read", "Grep", "Edit", "Write"])
    roll = rng.random()
    # Mostly small, a few long, now and then a huge log.
    size = rng.randint(200_000, 800_000) if roll < 0.003 else rng.randint(8_000, 40_000) if roll < 0.15 else rng.randint(60, 3_000)
    if name == "Edit":
        tool_input = {"file_path": f"/tmp/project/{rng.choice(words)}.swift", "old_string": sentence(6), "new_string": sentence(9)}
    elif name == "Write":
        tool_input = {"file_path": f"/tmp/project/{rng.choice(words)}.swift", "content": "\n".join(sentence(8) for _ in range(40))}
    elif name == "Bash":
        tool_input = {"command": f"swift test --filter {rng.choice(words)}", "description": sentence(4)}
    else:
        tool_input = {"file_path": f"/tmp/project/{rng.choice(words)}.swift"}
    line = sentence(12)
    return {"id": "toolu_" + uid(), "kind": "tool", "text": "", "toolName": name, "toolInput": json.dumps(tool_input),
            "toolResult": "\n".join(line for _ in range(max(1, size // len(line)))), "date": stamp(day, minute)}

def huge_thread(day, size):
    entries, minute = [], 0
    compactions = max(1, int(30 * scale * size))
    for compaction in range(compactions):
        for _ in range(3):
            minute += 3
            entries.append({"id": uid(), "kind": "user", "text": sentence(rng.randint(5, 40)), "date": stamp(day, minute),
                            "turnEndedAt": stamp(day, minute + 2)})
            # A turn of a few hundred tool calls, thinking and words between them.
            for _ in range(rng.randint(80, 150)):
                if rng.random() < 0.6:
                    entries.append({"id": uid(), "kind": "thinking", "text": " ".join(sentence(20) for _ in range(rng.randint(1, 8))), "date": stamp(day, minute)})
                for _ in range(rng.randint(1, 3)):
                    entries.append(tool(day, minute))
                if rng.random() < 0.5:
                    entries.append({"id": uid(), "kind": "assistant", "text": answer(), "date": stamp(day, minute)})
            entries.append({"id": uid(), "kind": "assistant", "text": answer() + "\n\n" + answer(), "date": stamp(day, minute + 2)})
        entries.append({"id": uid(), "kind": "notice", "text": f"Conversation compacted automatically · {rng.randint(180, 200)}k → {rng.randint(10, 30)}k tokens",
                        "date": stamp(day, minute)})
    return entries

projects = [{"id": uid(), "name": name, "path": f"{{{{ROOT}}}}/{name.lower()}", "permissionMode": "auto"}
            for name in ["Home", "Alpha", "Beta"]]
os.makedirs(f"{out}/threads", exist_ok=True)
for project in projects:
    os.makedirs(f"{out}/{project['name'].lower()}", exist_ok=True)
threads, total = [], 0
for index in range(20):
    id = uid()
    entries = huge_thread(55, 1 if index < 4 else 0.5)
    total += len(entries)
    with open(f"{out}/threads/{id}.json", "w") as file:
        json.dump(entries, file)
    threads.append({"id": id, "projectID": projects[0]["id"], "title": f"Huge {index + 1:02d}", "createdAt": stamp(55, index),
                    "updatedAt": stamp(55, 60 - index), "model": "haiku", "sessionID": str(uuid.UUID(int=rng.getrandbits(128), version=4))})
for index in range(100):
    id = uid()
    entries = [{"id": uid(), "kind": "user", "text": sentence(10), "date": stamp(10, index)},
               tool(10, index), {"id": uid(), "kind": "assistant", "text": answer(), "date": stamp(10, index)}]
    with open(f"{out}/threads/{id}.json", "w") as file:
        json.dump(entries, file)
    threads.append({"id": id, "projectID": projects[index % len(projects)]["id"], "title": sentence(4)[:-1],
                    "createdAt": stamp(10, 0), "updatedAt": stamp(10, index), "model": "haiku"})
state = {"projects": projects, "threads": threads, "selectedThreadID": threads[0]["id"], "homeProjectID": projects[0]["id"],
         "defaults": {"model": "haiku", "effort": "low"}, "titleGeneration": {"provider": "claude", "model": "haiku"},
         "autoSettleDays": 0, "browserAutoShow": False}
with open(f"{out}/state.json", "w") as file:
    json.dump(state, file)
print(f"{len(threads)} threads, 20 huge, {total} rows in those")
