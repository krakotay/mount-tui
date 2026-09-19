use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
}

impl SshHost {
    pub fn destination(&self) -> String {
        let host = self.hostname.as_deref().unwrap_or(&self.alias);
        self.user
            .as_ref()
            .map(|user| format!("{user}@{host}"))
            .unwrap_or_else(|| host.to_string())
    }

    pub fn source(&self, remote_path: &str) -> String {
        let path = if remote_path.trim().is_empty() {
            "/"
        } else {
            remote_path.trim()
        };
        format!("{}:{path}", self.destination())
    }
}

#[derive(Debug, Default, Clone)]
struct HostValues {
    hostname: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_file: Option<PathBuf>,
}

pub fn read_ssh_config(home: &Path) -> io::Result<Vec<SshHost>> {
    let path = home.join(".ssh/config");
    let mut visited = HashSet::new();
    match read_config_expanded(&path, home, &mut visited, 0) {
        Ok(contents) => Ok(parse_ssh_config(&contents, home)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn read_config_expanded(
    path: &Path,
    home: &Path,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
) -> io::Result<String> {
    if depth > 8 {
        return Ok(String::new());
    }
    let identity = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(identity) {
        return Ok(String::new());
    }
    let contents = fs::read_to_string(path)?;
    let mut expanded = String::new();
    for raw_line in contents.lines() {
        let line = strip_comment(raw_line).trim();
        let (key, value) = split_directive(line);
        if key.eq_ignore_ascii_case("include") {
            for pattern in value.split_whitespace().map(unquote) {
                for included in expand_include_pattern(pattern, home) {
                    if let Ok(contents) = read_config_expanded(&included, home, visited, depth + 1)
                    {
                        expanded.push_str(&contents);
                        if !contents.ends_with('\n') {
                            expanded.push('\n');
                        }
                    }
                }
            }
        } else {
            expanded.push_str(raw_line);
            expanded.push('\n');
        }
    }
    Ok(expanded)
}

fn expand_include_pattern(pattern: &str, home: &Path) -> Vec<PathBuf> {
    let path = pattern
        .strip_prefix("~/")
        .map(|rest| home.join(rest))
        .unwrap_or_else(|| {
            let path = PathBuf::from(pattern);
            if path.is_absolute() {
                path
            } else {
                home.join(".ssh").join(path)
            }
        });
    let Some(file_pattern) = path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    if !file_pattern.contains(['*', '?']) {
        return vec![path];
    }
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    let mut matches = fs::read_dir(parent)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| wildcard_matches(file_pattern, &entry.file_name().to_string_lossy()))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    matches.sort();
    matches
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut table = vec![vec![false; value.len() + 1]; pattern.len() + 1];
    table[0][0] = true;
    for index in 0..pattern.len() {
        match pattern[index] {
            b'*' => {
                for offset in 0..=value.len() {
                    table[index + 1][offset] =
                        table[index][offset] || (offset > 0 && table[index + 1][offset - 1]);
                }
            }
            b'?' => {
                for offset in 1..=value.len() {
                    table[index + 1][offset] = table[index][offset - 1];
                }
            }
            byte => {
                for offset in 1..=value.len() {
                    table[index + 1][offset] =
                        table[index][offset - 1] && byte == value[offset - 1];
                }
            }
        }
    }
    table[pattern.len()][value.len()]
}

pub fn parse_ssh_config(contents: &str, home: &Path) -> Vec<SshHost> {
    let mut global = HostValues::default();
    let mut hosts: BTreeMap<String, HostValues> = BTreeMap::new();
    let mut active = Vec::<String>::new();
    let mut in_match = false;
    let mut in_host = false;

    for raw_line in contents.lines() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = split_directive(line);
        let key = key.to_ascii_lowercase();
        if key == "match" {
            active.clear();
            in_match = true;
            in_host = false;
            continue;
        }
        if key == "host" {
            in_match = false;
            in_host = true;
            active = value
                .split_whitespace()
                .filter(|alias| !alias.starts_with('!') && !alias.contains(['*', '?']))
                .map(str::to_string)
                .collect();
            for alias in &active {
                hosts.entry(alias.clone()).or_insert_with(|| global.clone());
            }
            continue;
        }
        if in_match {
            continue;
        }
        if !in_host {
            apply_value(&mut global, &key, value, home);
        } else if !active.is_empty() {
            for alias in &active {
                if let Some(host) = hosts.get_mut(alias) {
                    apply_value(host, &key, value, home);
                }
            }
        }
    }

    hosts
        .into_iter()
        .map(|(alias, values)| SshHost {
            alias,
            hostname: values.hostname,
            user: values.user,
            port: values.port,
            identity_file: values.identity_file,
        })
        .collect()
}

fn apply_value(values: &mut HostValues, key: &str, raw_value: &str, home: &Path) {
    let value = unquote(raw_value.trim());
    match key {
        "hostname" if values.hostname.is_none() => values.hostname = Some(value.to_string()),
        "user" if values.user.is_none() => values.user = Some(value.to_string()),
        "port" if values.port.is_none() => values.port = value.parse().ok(),
        "identityfile" if values.identity_file.is_none() => {
            let expanded = value
                .strip_prefix("~/")
                .map(|rest| home.join(rest))
                .unwrap_or_else(|| PathBuf::from(value));
            values.identity_file = Some(expanded);
        }
        _ => {}
    }
}

fn split_directive(line: &str) -> (&str, &str) {
    if let Some((key, value)) = line.split_once('=') {
        return (key.trim(), value.trim());
    }
    let split = line.find(char::is_whitespace).unwrap_or(line.len());
    (&line[..split], line[split..].trim())
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}

fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    for (index, ch) in line.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..index],
            _ => {}
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_concrete_hosts_and_expands_identity_files() {
        let hosts = parse_ssh_config(
            r#"
                User deploy
                Host *.internal
                    Port 2200
                Host media media-backup
                    HostName storage.example.com
                    User alice
                    Port 2222
                    IdentityFile "~/.ssh/media key"
                Match host media
                    User ignored
            "#,
            Path::new("/home/alice"),
        );
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].alias, "media");
        assert_eq!(hosts[0].destination(), "deploy@storage.example.com");
        assert_eq!(hosts[0].port, Some(2222));
        assert_eq!(
            hosts[0].identity_file.as_deref(),
            Some(Path::new("/home/alice/.ssh/media key"))
        );
        assert_eq!(hosts[1].alias, "media-backup");
    }

    #[test]
    fn wildcard_matching_supports_common_include_patterns() {
        assert!(wildcard_matches("*.conf", "work.conf"));
        assert!(wildcard_matches("host?", "host1"));
        assert!(!wildcard_matches("*.conf", "work.txt"));
    }
}
