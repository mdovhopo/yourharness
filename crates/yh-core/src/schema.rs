//! The schema of `harness.sqlite`, step by step, verbatim the steps 0.0.x runs, so
//! both open the same database (0.0.16 still opens it after a downgrade). Each step runs once, in order; `PRAGMA user_version` records how
//! far a database has got. Append, never edit.

pub const STEPS: &[&str] = &[
    // 1: the first database.
    r#"CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value BLOB NOT NULL
) WITHOUT ROWID;

CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    position INTEGER NOT NULL,
    json BLOB NOT NULL
) WITHOUT ROWID;

CREATE TABLE threads (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    updated_at REAL,
    session_id TEXT,
    json BLOB NOT NULL
);
CREATE INDEX threads_project ON threads(project_id);
CREATE INDEX threads_session ON threads(session_id) WHERE session_id IS NOT NULL;

CREATE TABLE entries (
    thread_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    id TEXT NOT NULL,
    kind TEXT NOT NULL,
    text TEXT NOT NULL,
    tool_name TEXT,
    tool_input TEXT,
    tool_input_blob TEXT,
    tool_result TEXT,
    tool_result_blob TEXT,
    tool_result_length INTEGER,
    tool_is_error INTEGER,
    notice_style TEXT,
    attachments TEXT,
    delivery TEXT,
    date REAL,
    turn_ended_at REAL,
    subagent BLOB,
    subagent_blob TEXT,
    rev INTEGER NOT NULL,
    UNIQUE (thread_id, seq)
);
CREATE INDEX entries_rev ON entries(rev);
CREATE INDEX entries_entry ON entries(thread_id, id);
CREATE INDEX entries_input_blob ON entries(tool_input_blob) WHERE tool_input_blob IS NOT NULL;
CREATE INDEX entries_result_blob ON entries(tool_result_blob) WHERE tool_result_blob IS NOT NULL;
CREATE INDEX entries_subagent_blob ON entries(subagent_blob) WHERE subagent_blob IS NOT NULL;

CREATE TABLE transcripts (
    thread_id TEXT PRIMARY KEY,
    saved_at REAL NOT NULL,
    entry_count INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE blobs (
    hash TEXT PRIMARY KEY,
    size INTEGER NOT NULL,
    codec INTEGER NOT NULL,
    data BLOB NOT NULL
) WITHOUT ROWID;

CREATE VIRTUAL TABLE entries_fts USING fts5(body, tokenize = 'unicode61 remove_diacritics 2');

CREATE TABLE index_state (
    indexer TEXT PRIMARY KEY,
    rev INTEGER NOT NULL,
    updated_at REAL
) WITHOUT ROWID;

CREATE TABLE legacy_imports (
    name TEXT PRIMARY KEY,
    imported_at REAL NOT NULL
) WITHOUT ROWID;

CREATE TABLE memories (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL,
    rev INTEGER NOT NULL
);
CREATE INDEX memories_scope ON memories(scope, status);

CREATE TABLE memory_sources (
    memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    thread_id TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    quote TEXT,
    PRIMARY KEY (memory_id, thread_id, entry_id)
) WITHOUT ROWID;
CREATE INDEX memory_sources_thread ON memory_sources(thread_id);

CREATE VIRTUAL TABLE memories_fts USING fts5(title, body, content = 'memories', content_rowid = 'rowid',
                                             tokenize = 'unicode61 remove_diacritics 2');
CREATE TRIGGER memories_fts_insert AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
END;
CREATE TRIGGER memories_fts_delete AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, title, body) VALUES ('delete', old.rowid, old.title, old.body);
END;
CREATE TRIGGER memories_fts_update AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, title, body) VALUES ('delete', old.rowid, old.title, old.body);
    INSERT INTO memories_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
END;"#,
    // 2: memory you can explore: a one-line description, pins, where each came from,
    // links between memories, every version, and what agents do with them.
    r#"ALTER TABLE memories ADD COLUMN description TEXT NOT NULL DEFAULT '';
ALTER TABLE memories ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memories ADD COLUMN origin TEXT;
ALTER TABLE memories ADD COLUMN recall_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memories ADD COLUMN surfaced_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memories ADD COLUMN last_recalled_at REAL;
ALTER TABLE memories ADD COLUMN merged_into TEXT;

CREATE TABLE memory_links (
    from_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    to_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    relation TEXT,
    created_at REAL NOT NULL,
    PRIMARY KEY (from_id, to_id)
) WITHOUT ROWID;
CREATE INDEX memory_links_to ON memory_links(to_id);

CREATE TABLE memory_versions (
    memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    body TEXT NOT NULL,
    status TEXT NOT NULL,
    saved_at REAL NOT NULL,
    actor TEXT,
    thread_id TEXT,
    PRIMARY KEY (memory_id, version)
) WITHOUT ROWID;

CREATE TABLE memory_events (
    id INTEGER PRIMARY KEY,
    scope TEXT NOT NULL,
    memory_id TEXT,
    title TEXT,
    kind TEXT NOT NULL,
    actor TEXT,
    thread_id TEXT,
    at REAL NOT NULL,
    query TEXT,
    count INTEGER,
    detail TEXT
);
CREATE INDEX memory_events_scope ON memory_events(scope, at);
CREATE INDEX memory_events_memory ON memory_events(memory_id, at) WHERE memory_id IS NOT NULL;

DROP TRIGGER memories_fts_insert;
DROP TRIGGER memories_fts_delete;
DROP TRIGGER memories_fts_update;
DROP TABLE memories_fts;
CREATE VIRTUAL TABLE memories_fts USING fts5(title, description, body, content = 'memories', content_rowid = 'rowid',
                                             tokenize = 'unicode61 remove_diacritics 2');
CREATE TRIGGER memories_fts_insert AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, title, description, body) VALUES (new.rowid, new.title, new.description, new.body);
END;
CREATE TRIGGER memories_fts_delete AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, title, description, body)
    VALUES ('delete', old.rowid, old.title, old.description, old.body);
END;
CREATE TRIGGER memories_fts_update AFTER UPDATE OF title, description, body ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, title, description, body)
    VALUES ('delete', old.rowid, old.title, old.description, old.body);
    INSERT INTO memories_fts(rowid, title, description, body) VALUES (new.rowid, new.title, new.description, new.body);
END;
INSERT INTO memories_fts(memories_fts) VALUES ('rebuild');"#,
    // 3: tasks, the user's asks and how far they've got.
    r#"ALTER TABLE memories ADD COLUMN task_state TEXT;
ALTER TABLE memories ADD COLUMN task_steps TEXT;
ALTER TABLE memory_versions ADD COLUMN task_state TEXT;
ALTER TABLE memory_versions ADD COLUMN task_steps TEXT;
CREATE INDEX memories_tasks ON memories(scope, task_state) WHERE task_state IS NOT NULL;"#,
    // 4: on the message that started a turn, the files the turn changed, as git saw them.
    r#"ALTER TABLE entries ADD COLUMN changed_files TEXT;"#,
];
