//! [`GitCliProvider`] — `std::process::Command` backend for [`GitProvider`].
//!
//! This is the v0 default Git access path. It shells out to the system
//! `git` binary for every operation, parses textual output, and enforces a
//! per-call timeout (default 30s) implemented with a polling `try_wait` loop
//! plus parallel stdout/stderr drain threads. The pure-std implementation
//! avoids pulling in `wait_timeout`, `tokio`, or `crossbeam` for what is
//! always a short-lived subprocess.
//!
//! ## Read-only contract
//!
//! Every method invokes a `git` subcommand from a small allowlist of
//! read-only operations (`rev-parse`, `show-ref`, `for-each-ref`, `log`,
//! `show`, `check-mailmap`, `cat-file -e`). The integration test
//! `tests/no_git_write_subcommand.rs` intercepts every git invocation via a
//! PATH shim and asserts that no destructive subcommand is ever issued.
//!
//! ## Log walk format
//!
//! [`GitCliProvider::walk_commits`] invokes:
//!
//! ```text
//! git log --pretty=format:'%x02%H%x1f%an%x1f%ae%x1f%cn%x1f%ce%x1f%at%x1f%ct%x1f%s%x1f%P%x1f%b%x03'
//!         --name-status --numstat <from..to>
//! ```
//!
//! `\x02` (STX) frames the start of each commit record and `\x03` (ETX)
//! terminates the body field, so commit bodies containing arbitrary
//! newlines parse unambiguously. `\x1f` (US) separates fields within the
//! header. The literal `--pretty=format:` string and `\x1f` field separator
//! match the M3 task spec; the `\x02` / `\x03` framing and trailing `%b`
//! are required additions because co-author trailers must be parsed from
//! the body (SPEC-001 §4.4) and the original field-only format cannot frame
//! a multi-line body unambiguously.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::git::{
    parse_coauthor_trailers, top_level_dirs, FileChange, GitProvider, MailmapResolved, RawCommit,
    RefEntry, RefScope, RefType, Sha, ShowOpts, WalkRange,
};

/// Default per-call subprocess timeout. SPEC-001 §4.4 + TDD-000 §2.2.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// `\x02` (STX) — record start marker for the `walk_commits` log format.
const RECORD_START: u8 = 0x02;
/// `\x03` (ETX) — terminates the `%b` (body) field in the log format.
const BODY_END: u8 = 0x03;
/// `\x1f` (US) — field separator within a commit header.
const FIELD_SEP: char = '\x1f';

const LOG_FORMAT: &str = "%x02%H%x1f%an%x1f%ae%x1f%cn%x1f%ce%x1f%at%x1f%ct%x1f%s%x1f%P%x1f%b%x03";

/// CLI-backed [`GitProvider`] implementation.
///
/// Construct via [`GitCliProvider::new`] (default 30s timeout) or
/// [`GitCliProvider::with_timeout`] for tests that need shorter caps.
#[derive(Debug, Clone)]
pub struct GitCliProvider {
    /// Working tree root the `git` binary is invoked from.
    repo_root: PathBuf,
    /// Per-call wall-clock timeout. Reaching the cap kills the child and
    /// returns [`Error::Git`] with `code = -1`.
    timeout: Duration,
}

impl GitCliProvider {
    /// Build a provider rooted at `repo_root` with the default 30s timeout.
    pub fn new(repo_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Build a provider with an explicit per-call timeout.
    pub fn with_timeout(repo_root: impl Into<PathBuf>, timeout: Duration) -> Self {
        Self {
            repo_root: repo_root.into(),
            timeout,
        }
    }

    /// Working-tree root the provider operates against.
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// Per-call subprocess timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Run `git <args>` and return raw stdout. Maps non-zero exits to
    /// [`Error::Git`] and timeouts to `Error::Git { code: -1, .. }`.
    fn run_git(&self, args: &[&str]) -> Result<Vec<u8>> {
        let raw = self.run_git_raw(args)?;
        if !raw.success {
            return Err(Error::Git {
                stderr: String::from_utf8_lossy(&raw.stderr).into_owned(),
                code: raw.code.unwrap_or(-1),
            });
        }
        Ok(raw.stdout)
    }

    /// Run `git <args>` and return the raw outcome without mapping non-zero
    /// exits to [`Error::Git`]. Used by [`Self::cat_file_exists`], where
    /// non-zero exit is the success signal for "object does not exist".
    fn run_git_raw(&self, args: &[&str]) -> Result<RawOutcome> {
        let mut child = Command::new("git")
            .args(args)
            .current_dir(&self.repo_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdout_pipe = child
            .stdout
            .take()
            .expect("stdout was piped in spawn config");
        let stderr_pipe = child
            .stderr
            .take()
            .expect("stderr was piped in spawn config");

        let stdout_thread = thread::spawn(move || drain(stdout_pipe));
        let stderr_thread = thread::spawn(move || drain(stderr_pipe));

        let start = Instant::now();
        let status = loop {
            match child.try_wait()? {
                Some(s) => break s,
                None => {
                    if start.elapsed() >= self.timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(Error::Git {
                            stderr: format!(
                                "git command timed out after {:?}: git {}",
                                self.timeout,
                                args.join(" ")
                            ),
                            code: -1,
                        });
                    }
                    thread::sleep(Duration::from_millis(20));
                }
            }
        };

        let stdout = stdout_thread.join().map_err(|_| Error::Git {
            stderr: "stdout reader thread panicked".into(),
            code: -1,
        })?;
        let stderr = stderr_thread.join().map_err(|_| Error::Git {
            stderr: "stderr reader thread panicked".into(),
            code: -1,
        })?;

        Ok(RawOutcome {
            success: status.success(),
            code: status.code(),
            stdout,
            stderr,
        })
    }
}

/// Outcome of a `git` invocation before the non-zero-exit-to-error mapping.
struct RawOutcome {
    success: bool,
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Read a stream to end, returning whatever was buffered before any error.
fn drain<R: Read>(mut r: R) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = r.read_to_end(&mut buf);
    buf
}

impl GitProvider for GitCliProvider {
    fn common_dir(&self) -> Result<PathBuf> {
        let out = self.run_git(&["rev-parse", "--git-common-dir"])?;
        let s = String::from_utf8_lossy(&out).trim().to_string();
        if s.is_empty() {
            return Err(Error::NotARepo {
                path: self.repo_root.clone(),
            });
        }
        let path = PathBuf::from(&s);
        if path.is_absolute() {
            Ok(path)
        } else {
            Ok(self.repo_root.join(path))
        }
    }

    fn rev_parse(&self, refname: &str) -> Result<Sha> {
        // --verify ensures git emits exactly one SHA or a non-zero exit;
        // ^{commit} forces tag-deref so annotated tags resolve to a commit
        // rather than a tag object.
        let arg = format!("{refname}^{{commit}}");
        let raw = self.run_git_raw(&["rev-parse", "--verify", "--quiet", &arg])?;
        if !raw.success {
            return Err(Error::ShaNotFound {
                sha: refname.to_string(),
            });
        }
        let s = String::from_utf8_lossy(&raw.stdout).trim().to_string();
        Sha::new(s)
    }

    fn list_refs(&self, scope: RefScope) -> Result<Vec<RefEntry>> {
        let (prefix, ref_type) = match scope {
            RefScope::Heads => ("refs/heads/", RefType::Branch),
            RefScope::Remotes => ("refs/remotes/", RefType::RemoteBranch),
            RefScope::Tags => ("refs/tags/", RefType::Tag),
        };
        // for-each-ref with --format gives "sha\tname"; for tags we set
        // %(*objectname) which dereferences annotated tags to the commit,
        // falling back to %(objectname) for lightweight tags.
        let format = match scope {
            RefScope::Tags => "%(objectname)\t%(*objectname)\t%(refname)",
            _ => "%(objectname)\t%(refname)",
        };
        let out = self.run_git(&["for-each-ref", "--format", format, prefix])?;
        let text = String::from_utf8_lossy(&out);
        let mut refs = Vec::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            let (sha_text, name) = match scope {
                RefScope::Tags if parts.len() == 3 => {
                    let deref = parts[1];
                    let primary = parts[0];
                    let sha = if deref.is_empty() { primary } else { deref };
                    (sha, parts[2])
                }
                _ if parts.len() == 2 => (parts[0], parts[1]),
                _ => continue,
            };
            let sha = Sha::new(sha_text)?;
            refs.push(RefEntry {
                name: name.to_string(),
                sha,
                ref_type,
            });
        }
        Ok(refs)
    }

    fn walk_commits(&self, range: WalkRange) -> Result<Vec<RawCommit>> {
        let range_arg = match &range.from {
            Some(from) => format!("{}..{}", from.as_str(), range.to.as_str()),
            None => range.to.as_str().to_string(),
        };
        let mut args: Vec<String> = vec![
            "log".into(),
            format!("--pretty=format:{LOG_FORMAT}"),
            "--name-status".into(),
            "--numstat".into(),
            "--no-color".into(),
        ];
        if let Some(max) = range.max {
            args.push(format!("-n{max}"));
        }
        args.push(range_arg);
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let out = self.run_git(&args_ref)?;
        parse_log_output(&out)
    }

    fn show(&self, sha: &Sha, opts: ShowOpts) -> Result<String> {
        let color_arg = if opts.color {
            "--color=always"
        } else {
            "--color=never"
        };
        let mut args: Vec<&str> = vec!["show", color_arg];
        if opts.stat {
            args.push("--stat");
        }
        args.push(sha.as_str());
        let out = self.run_git(&args)?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    fn check_mailmap(&self, name: &str, email: &str) -> Result<MailmapResolved> {
        // check-mailmap accepts "Name <email>" on argv. Output is the
        // canonicalised form in the same shape.
        let arg = format!("{name} <{email}>");
        let out = self.run_git(&["check-mailmap", &arg])?;
        let text = String::from_utf8_lossy(&out).trim().to_string();
        let open = text.rfind('<').ok_or_else(|| Error::Git {
            stderr: format!("malformed check-mailmap output: {text}"),
            code: 0,
        })?;
        let close = text.rfind('>').ok_or_else(|| Error::Git {
            stderr: format!("malformed check-mailmap output: {text}"),
            code: 0,
        })?;
        if close <= open + 1 {
            return Err(Error::Git {
                stderr: format!("malformed check-mailmap output: {text}"),
                code: 0,
            });
        }
        let resolved_name = text[..open].trim().to_string();
        let resolved_email = text[open + 1..close].trim().to_string();
        Ok(MailmapResolved {
            name: resolved_name,
            email: resolved_email,
        })
    }

    fn cat_file_exists(&self, sha: &Sha) -> Result<bool> {
        let raw = self.run_git_raw(&["cat-file", "-e", sha.as_str()])?;
        Ok(raw.success)
    }
}

/// Parse the byte stream emitted by [`GitCliProvider::walk_commits`] into a
/// list of [`RawCommit`] values. Public for unit testing.
pub(crate) fn parse_log_output(bytes: &[u8]) -> Result<Vec<RawCommit>> {
    let mut commits = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // Each record begins at a RECORD_START byte. Skip any leading
        // whitespace / record-suffix newlines.
        while i < bytes.len() && bytes[i] != RECORD_START {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        i += 1; // past RECORD_START

        // Body ends at BODY_END. Everything in [i, body_end) is the
        // header + body section.
        let body_end = match find(bytes, BODY_END, i) {
            Some(end) => end,
            None => break,
        };
        let header_blob = &bytes[i..body_end];
        // After body_end (`\x03`), git appends `\n\n<file entries>\n` before
        // the next record. Find the next RECORD_START (or EOF) to bound the
        // diff section.
        let diff_start = body_end + 1;
        let diff_end = find(bytes, RECORD_START, diff_start).unwrap_or(bytes.len());
        let diff_blob = &bytes[diff_start..diff_end];

        let header_text = String::from_utf8_lossy(header_blob);
        let mut fields: Vec<&str> = header_text.split(FIELD_SEP).collect();
        // Last split chunk is `%b` (body). Take it out so we have exactly
        // 9 leading header fields.
        let body = fields.pop().unwrap_or("").to_string();
        if fields.len() != 9 {
            return Err(Error::Git {
                stderr: format!(
                    "malformed git log record: expected 9 header fields + body, got {} fields",
                    fields.len()
                ),
                code: 0,
            });
        }
        let sha = Sha::new(fields[0])?;
        let author_name = fields[1].to_string();
        let author_email = fields[2].to_string();
        let committer_name = fields[3].to_string();
        let committer_email = fields[4].to_string();
        let authored_at = parse_epoch(fields[5])?;
        let committed_at = parse_epoch(fields[6])?;
        let subject = fields[7].to_string();
        let parent_shas = parse_parents(fields[8])?;

        let diff_text = String::from_utf8_lossy(diff_blob);
        let files_changed = parse_diff_section(&diff_text);
        let dirs_touched = top_level_dirs(&files_changed);
        let coauthors = parse_coauthor_trailers(&body);

        commits.push(RawCommit {
            sha,
            author_name,
            author_email,
            committer_name,
            committer_email,
            authored_at,
            committed_at,
            subject,
            body,
            parent_shas,
            files_changed,
            dirs_touched,
            coauthors,
        });

        i = diff_end;
    }
    Ok(commits)
}

fn find(haystack: &[u8], needle: u8, from: usize) -> Option<usize> {
    haystack[from..]
        .iter()
        .position(|b| *b == needle)
        .map(|p| p + from)
}

fn parse_epoch(s: &str) -> Result<i64> {
    s.trim().parse::<i64>().map_err(|_| Error::Git {
        stderr: format!("malformed epoch in git log: {s:?}"),
        code: 0,
    })
}

fn parse_parents(s: &str) -> Result<Vec<Sha>> {
    let mut out = Vec::new();
    for p in s.split_ascii_whitespace() {
        out.push(Sha::new(p)?);
    }
    Ok(out)
}

/// Parse the `--name-status --numstat` block following a commit header.
///
/// The interleaving from git is:
///
/// ```text
/// <blank>
/// <name-status lines: STATUS\tpath[\trename-to]>
/// <blank>
/// <numstat lines: ins\tdel\tpath>
/// ```
///
/// We collect by path: name-status seeds the entry (status char), and
/// numstat fills in the insertion/deletion counts.
fn parse_diff_section(text: &str) -> Vec<FileChange> {
    use std::collections::BTreeMap;
    let mut by_path: BTreeMap<String, FileChange> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();

    for line in text.lines() {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.is_empty() {
            continue;
        }
        let first = parts[0];

        // Numstat lines: first column is digits or '-' (binary).
        let looks_numstat =
            !first.is_empty() && (first == "-" || first.chars().all(|c| c.is_ascii_digit()));
        if looks_numstat && parts.len() >= 3 {
            let ins = if parts[0] == "-" {
                0
            } else {
                parts[0].parse::<u64>().unwrap_or(0)
            };
            let del = if parts[1] == "-" {
                0
            } else {
                parts[1].parse::<u64>().unwrap_or(0)
            };
            // Path is the last field; for renames numstat uses
            // "ins\tdel\t{old => new}" in default mode but we requested
            // plain --numstat which keeps just the post-rename path.
            let path = parts[parts.len() - 1].to_string();
            let entry = by_path.entry(path.clone()).or_insert_with(|| {
                order.push(path.clone());
                FileChange {
                    path: path.clone(),
                    status: 'M',
                    insertions: 0,
                    deletions: 0,
                }
            });
            entry.insertions = ins;
            entry.deletions = del;
            continue;
        }

        // Name-status lines: first column is one letter (A/M/D/R/C/T/U/X)
        // optionally followed by a similarity score (R100, C75).
        let status_char = first.chars().next().unwrap_or('?').to_ascii_uppercase();
        if !matches!(status_char, 'A' | 'M' | 'D' | 'R' | 'C' | 'T' | 'U' | 'X') {
            continue;
        }
        let path = if matches!(status_char, 'R' | 'C') && parts.len() >= 3 {
            parts[2].to_string()
        } else if parts.len() >= 2 {
            parts[1].to_string()
        } else {
            continue;
        };
        let entry = by_path.entry(path.clone()).or_insert_with(|| {
            order.push(path.clone());
            FileChange {
                path: path.clone(),
                status: status_char,
                insertions: 0,
                deletions: 0,
            }
        });
        entry.status = status_char;
    }
    order
        .into_iter()
        .filter_map(|p| by_path.remove(&p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_log_blob(records: &[(&str, &str)]) -> Vec<u8> {
        // Each record: (header_with_field_seps_no_body, diff_section)
        // We frame as: \x02 <header> \x1f <body=""> \x03 \n\n <diff> \n
        let mut out = Vec::new();
        for (hdr, diff) in records {
            out.push(RECORD_START);
            out.extend_from_slice(hdr.as_bytes());
            out.push(0x1f);
            // empty body
            out.push(BODY_END);
            out.extend_from_slice(b"\n\n");
            out.extend_from_slice(diff.as_bytes());
            out.extend_from_slice(b"\n");
        }
        out
    }

    #[test]
    fn parse_single_commit_with_two_files() {
        let header = "0123456789abcdef0123456789abcdef01234567\u{1f}Alice\u{1f}alice@example.com\u{1f}Alice\u{1f}alice@example.com\u{1f}1700000000\u{1f}1700000010\u{1f}fix the thing\u{1f}";
        // Wait — header above has 8 field seps which is 9 fields (sha..parents-empty).
        // Diff: name-status + numstat for src/a.rs and docs/b.md
        let diff = "M\tsrc/a.rs\nA\tdocs/b.md\n\n3\t1\tsrc/a.rs\n10\t0\tdocs/b.md\n";
        let blob = build_log_blob(&[(header, diff)]);
        let commits = parse_log_output(&blob).expect("parses");
        assert_eq!(commits.len(), 1);
        let c = &commits[0];
        assert_eq!(c.sha.as_str(), "0123456789abcdef0123456789abcdef01234567");
        assert_eq!(c.author_name, "Alice");
        assert_eq!(c.subject, "fix the thing");
        assert_eq!(c.parent_shas.len(), 0);
        assert_eq!(c.files_changed.len(), 2);
        let a = c
            .files_changed
            .iter()
            .find(|f| f.path == "src/a.rs")
            .unwrap();
        assert_eq!(a.status, 'M');
        assert_eq!(a.insertions, 3);
        assert_eq!(a.deletions, 1);
        let b = c
            .files_changed
            .iter()
            .find(|f| f.path == "docs/b.md")
            .unwrap();
        assert_eq!(b.status, 'A');
        assert_eq!(b.insertions, 10);
        assert_eq!(b.deletions, 0);
        assert_eq!(c.dirs_touched, vec!["docs", "src"]);
    }

    #[test]
    fn parse_handles_binary_numstat_dashes() {
        let header = "abc1234\u{1f}A\u{1f}a@x\u{1f}A\u{1f}a@x\u{1f}1\u{1f}1\u{1f}s\u{1f}";
        let diff = "M\tlogo.png\n\n-\t-\tlogo.png\n";
        let blob = build_log_blob(&[(header, diff)]);
        let commits = parse_log_output(&blob).unwrap();
        let f = &commits[0].files_changed[0];
        assert_eq!(f.path, "logo.png");
        assert_eq!(f.insertions, 0);
        assert_eq!(f.deletions, 0);
    }

    #[test]
    fn parse_handles_two_commits() {
        let h1 = "aa11\u{1f}A\u{1f}a@x\u{1f}A\u{1f}a@x\u{1f}10\u{1f}10\u{1f}s1\u{1f}";
        let h2 = "bb22\u{1f}B\u{1f}b@x\u{1f}B\u{1f}b@x\u{1f}20\u{1f}20\u{1f}s2\u{1f}aa11";
        let blob = build_log_blob(&[
            (h1, "M\tone.txt\n\n1\t0\tone.txt\n"),
            (h2, "A\ttwo.txt\n\n2\t0\ttwo.txt\n"),
        ]);
        let commits = parse_log_output(&blob).unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha.as_str(), "aa11");
        assert_eq!(commits[1].sha.as_str(), "bb22");
        assert_eq!(commits[1].parent_shas, vec![Sha::new("aa11").unwrap()]);
    }

    #[test]
    fn parse_handles_body_with_coauthor() {
        let mut out = Vec::new();
        out.push(RECORD_START);
        out.extend_from_slice(b"cc33\x1fauth\x1fa@x\x1fauth\x1fa@x\x1f1\x1f1\x1fsubj\x1f");
        out.extend_from_slice(b""); // no parents
        out.push(0x1f); // separator before body
        out.extend_from_slice(b"body line 1\n\nCo-authored-by: Bob <b@x>\n");
        out.push(BODY_END);
        out.extend_from_slice(b"\n\nM\tfile.rs\n\n1\t0\tfile.rs\n");
        let commits = parse_log_output(&out).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].coauthors, vec![("Bob".into(), "b@x".into())]);
        assert!(commits[0].body.contains("body line 1"));
    }

    #[test]
    fn parse_empty_input_returns_empty() {
        assert!(parse_log_output(&[]).unwrap().is_empty());
    }

    #[test]
    fn parse_rejects_truncated_header() {
        // Only 3 fields instead of 9.
        let mut blob = Vec::new();
        blob.push(RECORD_START);
        blob.extend_from_slice(b"abc\x1fa\x1fb\x1f");
        blob.push(BODY_END);
        let err = parse_log_output(&blob).unwrap_err();
        assert_eq!(err.code(), "git");
    }

    #[test]
    fn provider_defaults_to_30s_timeout() {
        let p = GitCliProvider::new("/tmp/whatever");
        assert_eq!(p.timeout(), Duration::from_secs(30));
    }

    #[test]
    fn rename_status_takes_post_rename_path() {
        // Name-status "R100\told\tnew" + numstat "1\t1\tnew".
        let header = "dd44\u{1f}A\u{1f}a@x\u{1f}A\u{1f}a@x\u{1f}1\u{1f}1\u{1f}s\u{1f}";
        let diff = "R100\told.rs\tnew.rs\n\n1\t1\tnew.rs\n";
        let blob = build_log_blob(&[(header, diff)]);
        let commits = parse_log_output(&blob).unwrap();
        let f = &commits[0].files_changed[0];
        assert_eq!(f.path, "new.rs");
        assert_eq!(f.status, 'R');
        assert_eq!(f.insertions, 1);
        assert_eq!(f.deletions, 1);
    }
}
