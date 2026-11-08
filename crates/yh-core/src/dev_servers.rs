//! Local web servers you're running (a Vite, Next or Rails dev server…), for the browser panel to
//! offer: what's listening on a TCP port, from `lsof`, kept to processes that serve web pages.

use std::process::Command;

/// Commands that run dev servers, by the start of the name `lsof` gives (it cuts names at 9).
const SERVERS: [&str; 18] = ["node", "bun", "deno", "python", "Python", "ruby", "php", "java", "go", "vite", "next-serv", "esbuild", "hugo", "caddy", "nginx", "uvicorn", "gunicorn", "rails"];

#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub port: u16,
    pub command: String,
}

impl Server {
    pub fn url(&self) -> String {
        format!("http://localhost:{}", self.port)
    }
}

/// Parses `lsof -nP -iTCP -sTCP:LISTEN -Fcn`: `c<command>` then `n<address>:<port>` lines.
pub fn parse(output: &str) -> Vec<Server> {
    let mut out: Vec<Server> = vec![];
    let mut command = String::new();
    for line in output.lines() {
        if let Some(c) = line.strip_prefix('c') {
            command = c.to_string();
        } else if let Some(n) = line.strip_prefix('n') {
            let Some(port) = n.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) else { continue };
            if port >= 1024 && SERVERS.iter().any(|s| command.starts_with(s)) && !out.iter().any(|s| s.port == port) {
                out.push(Server { port, command: command.clone() });
            }
        }
    }
    out.sort_by_key(|s| s.port);
    out
}

/// What's listening now. Blocks briefly: call aside.
pub fn list() -> Vec<Server> {
    Command::new("/usr/sbin/lsof").args(["-nP", "-iTCP", "-sTCP:LISTEN", "-Fcn"]).output().map(|o| parse(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

/// The processes under `root` (a thread's `claude`, and what it started), from `ps -A -o pid=,ppid=`
/// lines: `root` first, at most 500.
pub fn descendants(ps: &str, root: u32) -> Vec<u32> {
    let mut children: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    for line in ps.lines() {
        let fields: Vec<u32> = line.split_whitespace().filter_map(|f| f.parse().ok()).collect();
        if let [pid, ppid] = fields[..] {
            children.entry(ppid).or_default().push(pid);
        }
    }
    let (mut pids, mut queue) = (vec![], vec![root]);
    while let Some(pid) = queue.pop() {
        if pids.len() >= 500 {
            break;
        }
        if !pids.contains(&pid) {
            pids.push(pid);
            queue.extend(children.get(&pid).into_iter().flatten());
        }
    }
    pids
}

/// The ports in `lsof -a -iTCP -sTCP:LISTEN -P -n -Fn -p …` output (`n*:3000`, `n[::1]:5173`), sorted.
pub fn listening_ports(lsof: &str) -> Vec<u16> {
    let mut ports: Vec<u16> = lsof.lines().filter_map(|l| l.strip_prefix('n')).filter_map(|n| n.rsplit(':').next()?.parse().ok()).collect();
    ports.sort();
    ports.dedup();
    ports
}

/// TCP ports the processes under `root` listen on: what a thread's `claude` started. Never a
/// scan of every port. Blocks briefly: call aside.
pub fn ports_under(root: u32) -> Vec<u16> {
    let Ok(ps) = Command::new("/bin/ps").args(["-A", "-o", "pid=,ppid="]).output() else { return vec![] };
    let pids = descendants(&String::from_utf8_lossy(&ps.stdout), root);
    let list = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    Command::new("/usr/sbin/lsof")
        .args(["-a", "-iTCP", "-sTCP:LISTEN", "-P", "-n", "-Fn", "-p", &list])
        .output()
        .map(|o| listening_ports(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// Local addresses a command printed (`http://localhost:5173/`, `http://127.0.0.1:8000/docs`),
/// `0.0.0.0` read as `localhost`, in the order printed.
pub fn printed_urls(output: &str) -> Vec<String> {
    let mut found = vec![];
    for (i, _) in output.match_indices("http") {
        let rest = &output[i..];
        let Some(after) = rest.strip_prefix("https://").or_else(|| rest.strip_prefix("http://")) else { continue };
        let scheme = &rest[..rest.len() - after.len()];
        let Some(host) = ["localhost", "127.0.0.1", "0.0.0.0", "[::1]"].into_iter().find(|h| after.starts_with(h)) else { continue };
        let Some(port_part) = after[host.len()..].strip_prefix(':') else { continue };
        let digits: String = port_part.chars().take_while(char::is_ascii_digit).collect();
        if !(2..=5).contains(&digits.len()) || digits.parse::<u16>().is_err() {
            continue;
        }
        let tail: String = port_part[digits.len()..].chars().take_while(|c| !c.is_whitespace() && !"'\"`)]>,".contains(*c)).collect();
        let host = if host == "0.0.0.0" { "localhost" } else { host };
        let mut url = format!("{scheme}{host}:{digits}{tail}");
        while url.ends_with('.') {
            url.pop();
        }
        found.push(url);
    }
    found
}

fn port_of(url: &str) -> Option<u16> {
    let after = url.split_once("://")?.1;
    let host_port = after.split('/').next()?;
    host_port.rsplit(':').next()?.parse().ok()
}

/// What a thread's browser offers when it has nothing open (the Swift app's `LocalServers`):
/// ports under its `claude`, then addresses its commands printed (newest output first), one per
/// port, at most six.
pub fn found(ports: &[u16], printed: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    for p in ports {
        if seen.insert(*p) {
            out.push(format!("http://localhost:{p}"));
        }
    }
    for url in printed {
        if let Some(p) = port_of(url) {
            if seen.insert(p) {
                out.push(url.clone());
            }
        }
    }
    out.truncate(6);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_the_process_tree() {
        let ps = "  1     0\n 10     1\n 20    10\n 21    10\n 30    20\n 40     1\n";
        let mut under = descendants(ps, 10);
        under.sort();
        assert_eq!(under, vec![10, 20, 21, 30]);
    }

    #[test]
    fn reads_listening_ports() {
        assert_eq!(listening_ports("p20\nf5\nn*:5173\nf6\nn[::1]:5173\np30\nn127.0.0.1:8000\n"), vec![5173, 8000]);
    }

    #[test]
    fn finds_printed_urls() {
        let out = "  VITE ready\n  ➜  Local:   http://localhost:5173/\n  Serving on http://0.0.0.0:8000. Docs at (http://127.0.0.1:8000/docs) and https://example.com:443";
        assert_eq!(printed_urls(out), vec!["http://localhost:5173/", "http://localhost:8000", "http://127.0.0.1:8000/docs"]);
        assert_eq!(found(&[3000, 5173], &printed_urls(out)), vec!["http://localhost:3000", "http://localhost:5173", "http://localhost:8000"]);
    }

    #[test]
    fn keeps_dev_servers() {
        let out = "p1\ncnode\nn*:5173\nn[::1]:5173\np2\ncControlCe\nn*:5000\np3\ncpython3.1\nn127.0.0.1:8000\np4\ncnode\nn*:80\n";
        assert_eq!(parse(out), vec![Server { port: 5173, command: "node".into() }, Server { port: 8000, command: "python3.1".into() }]);
    }
}
