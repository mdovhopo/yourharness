# Fills a data folder with a lot of history in the 0.0.6 JSON format: hundreds of threads,
# a few very long ones with big tool results. For timing the move into the database, the
# full-text index, thread switching and memory on real-sized data.
#   python3 generate.py <data folder> [scale]    (scale 1 is about 150 MB of JSON)
import json, os, random, sys, uuid

out = sys.argv[1]
scale = float(sys.argv[2]) if len(sys.argv) > 2 else float(os.environ.get("BIG_SCALE", "1"))
rng = random.Random(7)
words = ("parser migration index thread session memory window sidebar transcript render layout switch commit branch "
         "release build test flaky timezone cache budget token query search worktree review merge stack diagram").split()

def sentence(n):
    return " ".join(rng.choice(words) for _ in range(n)).capitalize() + "."

def uid():
    return str(uuid.UUID(int=rng.getrandbits(128), version=4)).upper()

def stamp(day, minute):
    return f"2026-{9 + day // 28:02d}-{day % 28 + 1:02d}T{minute // 60 % 24:02d}:{minute % 60:02d}:00Z"

def turn(index, day, big):
    entries = [{"id": uid(), "kind": "user", "text": sentence(rng.randint(5, 30)), "date": stamp(day, index),
                "turnEndedAt": stamp(day, index + 1)}]
    for step in range(rng.randint(1, 6)):
        tool = rng.choice(["Bash", "Read", "Grep", "Edit"])
        size = rng.randint(20_000, 120_000) if big and rng.random() < 0.3 else rng.randint(50, 2_000)
        result = "\n".join(sentence(12) for _ in range(size // 80))
        entries.append({"id": "toolu_" + uid(), "kind": "tool", "text": "", "toolName": tool,
                        "toolInput": json.dumps({"command": f"swift test --filter {rng.choice(words)}"} if tool == "Bash"
                                                else {"file_path": f"/tmp/project/{rng.choice(words)}.swift"}),
                        "toolResult": result, "date": stamp(day, index)})
    entries.append({"id": uid(), "kind": "assistant", "text": "\n\n".join(sentence(20) for _ in range(rng.randint(1, 5))),
                    "date": stamp(day, index)})
    return entries

projects = [{"id": uid(), "name": name, "path": f"{{{{ROOT}}}}/{name.lower()}", "permissionMode": "auto"}
            for name in ["Home", "Alpha", "Beta", "Gamma"]]
os.makedirs(f"{out}/threads", exist_ok=True)
threads = []
heavy = max(1, int(5 * scale))
for index in range(int(400 * scale)):
    long = index < heavy
    # The long ones are the newest in Home, so they head the sidebar (and YH_SWITCH_PROBE).
    id, project = uid(), projects[0] if long else projects[index % len(projects)]
    day = 55 if long else rng.randint(0, 50)
    turns = rng.randint(400, 600) if long else rng.randint(3, 15)
    entries = [entry for t in range(turns) for entry in turn(t, day, long)]
    with open(f"{out}/threads/{id}.json", "w") as file:
        json.dump(entries, file)
    threads.append({"id": id, "projectID": project["id"], "title": ("Long: " if long else "") + sentence(4)[:-1],
                    "createdAt": stamp(day, 0), "updatedAt": stamp(day, turns), "model": "haiku"})
for project in projects:
    os.makedirs(f"{out}/{project['name'].lower()}", exist_ok=True)
state = {"projects": projects, "threads": threads, "selectedThreadID": threads[0]["id"], "homeProjectID": projects[0]["id"],
         "defaults": {"model": "haiku", "effort": "low"}, "titleGeneration": {"provider": "claude", "model": "haiku"},
         "autoSettleDays": 0, "browserAutoShow": False}
with open(f"{out}/state.json", "w") as file:
    json.dump(state, file)
print(f"{len(threads)} threads, {heavy} long ones")
