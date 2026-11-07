//! What Claude left running in a thread: a command in the background (a build, a dev server),
//! a foreground one that's taking a while, or a watch (Monitor). Lives with the `claude`
//! process, never saved. The thread holds them; the strip above the composer shows them and
//! reads their output itself, once a second, so nothing here changes while they just run.

use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Completed,
    Failed,
    Stopped,
}

#[derive(Clone, Debug)]
pub struct Job {
    /// The task's ID, what `stop_task` takes.
    pub id: String,
    pub tool_id: Option<String>,
    pub description: String,
    /// Started in the background: the turn didn't wait for it.
    pub background: bool,
    /// A Monitor watch, which wakes Claude with what it sees, rather than a command.
    pub watch: bool,
    /// Your message it started under: while that's the latest, Claude is waiting on it.
    pub turn: Option<String>,
    pub started: Instant,
    pub status: JobStatus,
    pub ended: Option<Instant>,
    pub exit_code: Option<i32>,
    /// Everything it printed, as the CLI writes it, once known.
    pub output_file: Option<String>,
    /// What a watch last saw.
    pub last_line: Option<String>,
}

impl Job {
    /// "finished", "failed with exit code 1", "was stopped".
    pub fn outcome(&self) -> String {
        match self.status {
            JobStatus::Running => "is running".into(),
            JobStatus::Completed if self.watch => "ended".into(),
            JobStatus::Completed => "finished".into(),
            JobStatus::Failed => self.exit_code.map_or("failed".into(), |c| format!("failed with exit code {c}")),
            JobStatus::Stopped => "was stopped".into(),
        }
    }

    fn kind(&self) -> &'static str {
        if self.watch {
            "Watch"
        } else {
            "Background command"
        }
    }

    /// The note its end leaves in the thread: `Background command “Build” failed with exit
    /// code 1 after 35s`.
    pub fn end_note(&self) -> String {
        let took = self.ended.map(|e| format!(" after {}", format_duration(e.duration_since(self.started).as_secs_f64()))).unwrap_or_default();
        format!("{} “{}” {}{took}", self.kind(), self.description, self.outcome())
    }

    fn end(&mut self, status: JobStatus) {
        if self.status == JobStatus::Running {
            self.status = status;
            self.ended = Some(Instant::now());
        }
    }
}

/// "4s", "2m 14s", "1h 5m", as the Swift app says how long a turn took.
pub fn format_duration(seconds: f64) -> String {
    let total = seconds.round() as u64;
    if total < 60 {
        format!("{}s", total.max(1))
    } else if total < 3600 {
        format!("{}m {}s", total / 60, total % 60)
    } else {
        format!("{}h {}m", total / 3600, (total % 3600) / 60)
    }
}

/// How long it's been running, as a clock: "0:07", "12:30", "1:02:03".
pub fn clock(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A thread's jobs. `version` goes up with every change the strip shows.
#[derive(Default)]
pub struct Jobs {
    /// Still running, oldest first.
    pub running: Vec<Job>,
    pub version: u64,
    /// Where this session's jobs write their output, once one has said.
    pub folder: Option<String>,
    /// Your latest message's ID.
    pub turn: Option<String>,
}

impl Jobs {
    pub fn start(&mut self, task_id: String, tool_id: Option<String>, tool_name: Option<&str>, description: String, background: bool) {
        if self.running.iter().any(|j| j.id == task_id) {
            return;
        }
        let output_file = self.folder.as_ref().map(|f| format!("{f}/{task_id}.output"));
        self.running.push(Job {
            id: task_id,
            tool_id,
            description,
            background,
            watch: tool_name == Some("Monitor"),
            turn: self.turn.clone(),
            started: Instant::now(),
            status: JobStatus::Running,
            ended: None,
            exit_code: None,
            output_file,
            last_line: None,
        });
        self.version += 1;
    }

    /// The CLI's word that a job ended. The note to leave in the thread, for a background one
    /// (a foreground command shows how it went in its own call).
    pub fn finish(&mut self, task_id: &str, status: &str, exit_code: Option<i32>, output_file: Option<String>) -> Option<String> {
        let ix = self.running.iter().position(|j| j.id == task_id)?;
        let mut job = self.running.remove(ix);
        job.exit_code = exit_code;
        job.end(match status {
            "completed" if exit_code.is_some_and(|c| c != 0) => JobStatus::Failed,
            "completed" => JobStatus::Completed,
            "failed" | "error" => JobStatus::Failed,
            _ => JobStatus::Stopped,
        });
        if output_file.is_some() {
            job.output_file = output_file;
        }
        self.version += 1;
        job.background.then(|| job.end_note())
    }

    /// What a watch saw: its last line.
    pub fn saw(&mut self, task_id: &str, text: &str) {
        if let Some(job) = self.running.iter_mut().find(|j| j.id == task_id) {
            job.last_line = text.lines().rev().find(|l| !l.trim().is_empty()).map(str::to_string).or(job.last_line.take());
            self.version += 1;
        }
    }

    /// Every background job still running, by the CLI's list: the ones not on it ended
    /// without word. Notes for them.
    pub fn keep_only(&mut self, running: &[String]) -> Vec<String> {
        let (gone, kept): (Vec<Job>, Vec<Job>) = std::mem::take(&mut self.running).into_iter().partition(|j| j.background && !running.contains(&j.id));
        self.running = kept;
        if gone.is_empty() {
            return vec![];
        }
        self.version += 1;
        gone.into_iter()
            .map(|mut j| {
                j.end(JobStatus::Stopped);
                j.end_note()
            })
            .collect()
    }

    /// A tool call came back: a foreground command is done when its call is; a background
    /// one's result says where the session's jobs write their output.
    pub fn tool_result(&mut self, tool_id: &str, text: &str) {
        if let Some(file) = output_file_in_result(text) {
            self.folder = std::path::Path::new(&file).parent().map(|p| p.to_string_lossy().into_owned());
            if let Some(job) = self.running.iter_mut().find(|j| j.tool_id.as_deref() == Some(tool_id)) {
                job.output_file = Some(file);
            }
            self.version += 1;
        }
        let before = self.running.len();
        self.running.retain(|j| j.background || j.tool_id.as_deref() != Some(tool_id));
        if self.running.len() != before {
            self.version += 1;
        }
    }

    /// The process they ran in is going: they go with it. Notes for the background ones.
    pub fn cut_off(&mut self, outcome: &str) -> Vec<String> {
        if self.running.is_empty() {
            return vec![];
        }
        self.version += 1;
        std::mem::take(&mut self.running)
            .into_iter()
            .filter(|j| j.background)
            .map(|j| format!("{} “{}” {outcome}", j.kind(), j.description))
            .collect()
    }

    /// Background jobs started since your latest message: Claude ended its turn but is back
    /// when they report. Older ones (a dev server from earlier) just run.
    pub fn waiting(&self) -> usize {
        self.running.iter().filter(|j| j.background && j.turn == self.turn).count()
    }

    pub fn background(&self) -> impl Iterator<Item = &Job> {
        self.running.iter().filter(|j| j.background)
    }
}

/// Where the CLI writes a job's output, from what it said about one ("Output is being
/// written to: …/tasks/b1.output."): a foreground command's own path is never told.
pub fn output_file_in_result(result: &str) -> Option<String> {
    // A line or two: every other result, however long, needn't be searched.
    if result.len() >= 2048 {
        return None;
    }
    let rest = &result[result.find("Output is being written to: ")? + "Output is being written to: ".len()..];
    Some(rest[..rest.find(".output")? + ".output".len()].to_string())
}

/// Where the CLI keeps a session's job output when none has said yet:
/// `/private/tmp/claude-<uid>/<folder, dashed>/<session>/tasks`.
pub fn output_folder(folder: &str, session: &str) -> String {
    let real = std::fs::canonicalize(folder).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| folder.to_string());
    let dashed: String = real.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let uid = unsafe { libc::getuid() };
    format!("/private/tmp/claude-{uid}/{dashed}/{session}/tasks")
}

/// The last `bytes` of what a job printed, without the CLI's `[exited with code 0]` line.
/// Reads the file: off the main thread.
pub fn tail(path: &str, bytes: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(bytes))).ok()?;
    let mut data = vec![];
    file.read_to_end(&mut data).ok()?;
    let mut text = String::from_utf8_lossy(&data).into_owned();
    if size > bytes {
        if let Some(newline) = text.find('\n') {
            text.drain(..=newline);
        }
    }
    let lines: Vec<&str> = text.lines().filter(|l| !l.starts_with("[exited with code ")).collect();
    Some(lines.join("\n").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jobs() -> Jobs {
        let mut jobs = Jobs { turn: Some("m1".into()), ..Default::default() };
        jobs.start("b1".into(), Some("t1".into()), Some("Bash"), "Build".into(), true);
        jobs.start("b2".into(), Some("t2".into()), Some("Bash"), "Loop".into(), false);
        jobs
    }

    #[test]
    fn a_failed_background_job_leaves_a_note() {
        let mut jobs = jobs();
        assert_eq!(jobs.waiting(), 1);
        let note = jobs.finish("b1", "failed", Some(3), Some("/tmp/x/tasks/b1.output".into())).unwrap();
        assert!(note.starts_with("Background command “Build” failed with exit code 3 after "), "{note}");
        assert_eq!(jobs.waiting(), 0);
        // A foreground one's end says nothing: its call shows it.
        assert_eq!(jobs.finish("b2", "completed", None, None), None);
        assert!(jobs.running.is_empty());
        assert_eq!(jobs.finish("b2", "completed", None, None), None);
    }

    #[test]
    fn a_foreground_command_ends_with_its_call() {
        let mut jobs = jobs();
        jobs.tool_result("t2", "tick 1\ntick 2");
        assert_eq!(jobs.running.iter().map(|j| j.id.as_str()).collect::<Vec<_>>(), ["b1"]);
        // A background one has only started.
        jobs.tool_result("t1", "Command running in background with ID: b1. Output is being written to: /private/tmp/c/s/tasks/b1.output. You will be notified.");
        assert_eq!(jobs.running[0].output_file.as_deref(), Some("/private/tmp/c/s/tasks/b1.output"));
        assert_eq!(jobs.folder.as_deref(), Some("/private/tmp/c/s/tasks"));
        jobs.start("b3".into(), None, None, "Next".into(), true);
        assert_eq!(jobs.running[1].output_file.as_deref(), Some("/private/tmp/c/s/tasks/b3.output"));
    }

    #[test]
    fn ended_without_word_and_cut_off() {
        let mut jobs = jobs();
        assert!(jobs.keep_only(&["b1".into()]).is_empty());
        let notes = jobs.keep_only(&[]);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].starts_with("Background command “Build” was stopped"), "{}", notes[0]);
        // The foreground one isn't on the CLI's list: it ends with its call.
        assert_eq!(jobs.running.len(), 1);
        let mut jobs = self::jobs();
        assert_eq!(jobs.cut_off("stopped when YourHarness quit"), ["Background command “Build” stopped when YourHarness quit"]);
        assert!(jobs.running.is_empty());
    }

    #[test]
    fn older_jobs_just_run() {
        let mut jobs = jobs();
        jobs.turn = Some("m2".into());
        assert_eq!(jobs.waiting(), 0);
        assert_eq!(jobs.background().count(), 1);
    }

    #[test]
    fn a_watch_keeps_what_it_last_saw() {
        let mut jobs = Jobs::default();
        jobs.start("w".into(), None, Some("Monitor"), "Load".into(), true);
        jobs.saw("w", "sample 1: 2.58\nsample 2: 2.69\n");
        assert_eq!(jobs.running[0].last_line.as_deref(), Some("sample 2: 2.69"));
        assert_eq!(jobs.finish("w", "completed", None, None).map(|n| n.starts_with("Watch “Load” ended after")), Some(true));
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(0.2), "1s");
        assert_eq!(format_duration(35.0), "35s");
        assert_eq!(format_duration(134.0), "2m 14s");
        assert_eq!(format_duration(3900.0), "1h 5m");
        assert_eq!(clock(7), "0:07");
        assert_eq!(clock(750), "12:30");
        assert_eq!(clock(3723), "1:02:03");
    }

    #[test]
    fn the_tail_drops_the_exit_line() {
        let dir = std::env::temp_dir().join(format!("yh-jobs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("b.output");
        std::fs::write(&file, "one\ntwo\nthree\n[exited with code 0]\n").unwrap();
        assert_eq!(tail(file.to_str().unwrap(), 8192).as_deref(), Some("one\ntwo\nthree"));
        // Cut mid-line: the partial first line goes.
        assert_eq!(tail(file.to_str().unwrap(), 28).as_deref(), Some("three"));
        assert_eq!(tail("/nonexistent/b.output", 10), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
