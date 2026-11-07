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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_dev_servers() {
        let out = "p1\ncnode\nn*:5173\nn[::1]:5173\np2\ncControlCe\nn*:5000\np3\ncpython3.1\nn127.0.0.1:8000\np4\ncnode\nn*:80\n";
        assert_eq!(parse(out), vec![Server { port: 5173, command: "node".into() }, Server { port: 8000, command: "python3.1".into() }]);
    }
}
