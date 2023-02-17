//! The gridlock tool itself. This component manages lock files and the
//! updating thereof.

use std::{
    collections::{BTreeMap, HashMap},
    io::{Cursor, Read},
    path::Path,
    process::Stdio,
};

use async_trait::async_trait;
use chrono::Utc;
use color_eyre::eyre::{eyre, Context};
use regex::Regex;
use serde::{de::Visitor, Deserialize, Serialize, Serializer};
use serde_json::Value;
use tokio::{fs, io::AsyncWriteExt};

const LOCKFILE_VERSION: u16 = 0;

/// Lockfile format, loosely based on Niv's format, since it's simple and
/// mostly a good design.
#[derive(Deserialize, Serialize)]
pub struct Lockfile {
    pub packages: BTreeMap<String, Lock>,
    pub version: u16,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl Default for Lockfile {
    fn default() -> Self {
        Lockfile {
            packages: Default::default(),
            version: LOCKFILE_VERSION,
            extra: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UnixTimestamp(pub chrono::DateTime<Utc>);

impl<'de> Deserialize<'de> for UnixTimestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct MyVisitor {}

        impl<'de> Visitor<'de> for MyVisitor {
            type Value = UnixTimestamp;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("UnixTimestamp")
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(UnixTimestamp(chrono::DateTime::from_utc(
                    chrono::NaiveDateTime::from_timestamp_opt(v, 0).ok_or(
                        serde::de::Error::invalid_value(
                            serde::de::Unexpected::Signed(v),
                            &"a timestamp considered valid by chrono",
                        ),
                    )?,
                    Utc,
                )))
            }

            // since serde_json tries u64 first and then type errors (lol)
            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                self.visit_i64(v as i64)
            }
        }

        deserializer.deserialize_i64(MyVisitor {})
    }
}

impl Serialize for UnixTimestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i64(self.0.timestamp())
    }
}

pub type GitRevision = String;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Lock {
    pub branch: String,
    pub owner: String,
    pub repo: String,
    pub rev: GitRevision,
    pub sha256: String,
    pub last_updated: Option<UnixTimestamp>,
    pub url: String,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

fn archive_url(owner: &str, repo: &str, rev: &str) -> String {
    format!("https://github.com/{owner}/{repo}/archive/{rev}.tar.gz")
}

/// Some implementation of a client to do online stuff with GitHub.
/// Installed as an extension/mocking point.
#[async_trait]
pub trait GitHubClient {
    async fn branch_head(
        &self,
        owner: &str,
        repo: &str,
        branch_name: Option<&str>,
    ) -> color_eyre::Result<(String, GitRevision)>;

    async fn create_lock(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        rev: &str,
    ) -> color_eyre::Result<Lock>;
}

pub struct OnlineGitHubClient {
    client: reqwest::Client,
}

impl OnlineGitHubClient {
    pub fn new() -> color_eyre::Result<OnlineGitHubClient> {
        Ok(OnlineGitHubClient {
            client: reqwest::Client::builder()
                .user_agent("gridlock/0.1")
                .build()?,
        })
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
enum GitLsRemoteLine {
    SymRef { target: String, name: String },
    Branch { rev: String, target: String },
}

/// ```notrust
/// » git ls-remote --symref . HEAD
/// ref: refs/heads/main    HEAD
/// 59f5c322b48409c4d6d08cecae50b663151b22ed        HEAD
/// ref: refs/remotes/origin/main   refs/remotes/origin/HEAD
/// 59f5c322b48409c4d6d08cecae50b663151b22ed        refs/remotes/origin/HEAD
/// ```
fn parse_git_ls_remote_line(input: &str) -> color_eyre::Result<GitLsRemoteLine> {
    lazy_static::lazy_static! {
        static ref REF_RE: Regex = Regex::new(r#"ref: ([^\s]+)\s+([^\s]+)"#).unwrap();
        static ref TIP_RE: Regex = Regex::new(r#"([0-9a-f]+)\s+([^\s]+)"#).unwrap();
    };

    if let Some(refs) = REF_RE.captures(input) {
        Ok(GitLsRemoteLine::SymRef {
            target: refs[1].to_string(),
            name: refs[2].to_string(),
        })
    } else if let Some(tip) = TIP_RE.captures(input) {
        Ok(GitLsRemoteLine::Branch {
            rev: tip[1].to_string(),
            target: tip[2].to_string(),
        })
    } else {
        Err(eyre!(
            "could not parse line of git ls-remote output: {input:?}"
        ))
    }
}

async fn git_branch_head(
    remote: &str,
    branch_name: Option<&str>,
) -> color_eyre::Result<(GitRevision, String)> {
    // not confident this is the right approach/will not get us hosed by
    // rate limits
    let proc = tokio::process::Command::new("git")
        .arg("ls-remote")
        .arg("--symref")
        .arg(remote)
        .arg(branch_name.unwrap_or("HEAD"))
        .stdout(Stdio::piped())
        .output()
        .await?;

    let parsed = std::str::from_utf8(&proc.stdout)
        .context("utf8 decode git ls-remote")?
        .lines()
        .map(parse_git_ls_remote_line)
        .collect::<color_eyre::Result<Vec<GitLsRemoteLine>>>()?;

    let def_branch = parsed
        .iter()
        .find_map(|l| match l {
            GitLsRemoteLine::SymRef { target, .. } => {
                Some(target.strip_prefix("refs/heads/").unwrap_or(target))
            }
            _ => None,
        })
        .map(|s| s.to_string());

    let val = parsed
        .iter()
        .find_map(|l| match l {
            GitLsRemoteLine::Branch { rev, .. } => Some(rev),
            _ => None,
        })
        .cloned()
        .ok_or_else(|| eyre!("didn't get a branch line in {parsed:?}"))?;

    let branch_name = match branch_name {
        Some(v) => v.to_string(),
        None => def_branch.ok_or_else(|| eyre!("no default branch name"))?,
    };

    Ok((val, branch_name))
}

#[async_trait]
impl GitHubClient for OnlineGitHubClient {
    async fn branch_head(
        &self,
        owner: &str,
        repo: &str,
        branch_name: Option<&str>,
    ) -> color_eyre::Result<(GitRevision, String)> {
        git_branch_head(&format!("https://github.com/{owner}/{repo}"), branch_name).await
    }

    async fn create_lock(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        rev: &str,
    ) -> color_eyre::Result<Lock> {
        let url = archive_url(owner, repo, rev);
        let resp = self.client.get(&url).send().await?.bytes().await?;
        let content = resp.to_vec();

        // FIXME: add a debug option to put this tarball on disk
        // fs::write("content.tar.gz", &content).await?;
        let mut decoder = flate2::read::GzDecoder::new(Cursor::new(&content));
        let mut content = Vec::new();
        decoder.read_to_end(&mut content)?;

        let mut hasher = nyarr::hash::NarHasher::new();
        nyarr::tar::tar_to_nar(Cursor::new(&content), &mut hasher).map_err(|e| eyre!(e))?;

        Ok(Lock {
            owner: owner.into(),
            repo: repo.into(),
            branch: branch.into(),
            rev: rev.into(),
            url,
            last_updated: Some(UnixTimestamp(Utc::now())),
            sha256: hasher.digest(),
            extra: Default::default(),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum LockfileChange {
    UpdateRev(String, GitRevision),
}

pub async fn plan_update<C: GitHubClient>(
    client: &C,
    lf: &Lockfile,
    item: Option<&str>, // FIXME(jade): add progress callback
) -> color_eyre::Result<Vec<LockfileChange>> {
    let mut changes = vec![];

    // XXX(jade): lol this is ridiculous
    let it = item
        .map(|v| {
            Box::new(std::iter::once((v.to_string(), lf.packages[v].clone())))
                as Box<dyn Iterator<Item = (String, Lock)>>
        })
        .unwrap_or(Box::new(
            lf.packages
                .iter()
                .map(|(a, b)| (a.to_owned(), b.to_owned())),
        ));

    for (name, lock) in it {
        let (branch_head, _branch_name) = client
            .branch_head(&lock.owner, &lock.repo, Some(&lock.branch))
            .await
            .context("getting branch head")?;
        if branch_head != lock.rev {
            changes.push(LockfileChange::UpdateRev(name.to_string(), branch_head))
        }
    }

    Ok(changes)
}

pub async fn read_lockfile(path: &Path) -> color_eyre::Result<Lockfile> {
    let content = fs::read(path).await.context("reading lockfile")?;
    let lockfile = serde_json::from_slice(&content)?;
    Ok(lockfile)
}

pub async fn write_lockfile(path: &Path, content: &Lockfile) -> color_eyre::Result<()> {
    let new_path = path.with_extension(".tmp");
    let mut h = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&new_path)
        .await?;
    let data = serde_json::to_vec(content)?;
    h.write_all(&data).await?;
    fs::rename(&new_path, path).await?;
    Ok(())
}

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;

    use super::*;

    type Owner = String;
    type Repo = String;
    type BranchName = String;

    struct MockGitHubClient {
        branch_maps: BTreeMap<(Owner, Repo), BTreeMap<BranchName, GitRevision>>,
    }

    #[async_trait]
    impl GitHubClient for MockGitHubClient {
        async fn branch_head(
            &self,
            owner: &str,
            repo: &str,
            branch_name: Option<&str>,
        ) -> color_eyre::Result<(crate::GitRevision, String)> {
            let tip = self
                .branch_maps
                .get(&(owner.to_string(), repo.to_string()))
                .ok_or_else(|| eyre!("unknown owner/repo {owner} {repo}"))?
                .get(branch_name.unwrap_or("main"))
                .ok_or_else(|| eyre!("unknown branch {branch_name:?}"))
                .cloned()?;

            Ok((
                tip,
                branch_name
                    .map(|s| s.to_string())
                    .unwrap_or("main".to_string()),
            ))
        }

        async fn create_lock(
            &self,
            _owner: &str,
            _repo: &str,
            _branch: &str,
            _rev: &str,
        ) -> color_eyre::Result<Lock> {
            todo!()
        }
    }

    fn gh_client() -> MockGitHubClient {
        MockGitHubClient {
            branch_maps: BTreeMap::from_iter([
                (
                    ("lf-".into(), "aiobspwm".into()),
                    BTreeMap::from_iter([
                        (
                            "HEAD".into(),
                            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                        ),
                        (
                            "main".into(),
                            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                        ),
                        (
                            "branch".into(),
                            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
                        ),
                    ]),
                ),
                (
                    ("lf-".into(), "aiopanel".into()),
                    BTreeMap::from_iter([
                        (
                            "HEAD".into(),
                            "cccccccccccccccccccccccccccccccccccccccc".into(),
                        ),
                        (
                            "main".into(),
                            "cccccccccccccccccccccccccccccccccccccccc".into(),
                        ),
                        (
                            "branch".into(),
                            "dddddddddddddddddddddddddddddddddddddddd".into(),
                        ),
                    ]),
                ),
            ]),
        }
    }

    fn lock_file() -> Lockfile {
        let content = include_bytes!("testdata/lockfile.json");
        serde_json::from_slice(content).unwrap()
    }

    #[tokio::test]
    async fn test_plan_update() {
        let client = gh_client();
        let lf = lock_file();
        let changes = plan_update(&client, &lf, None).await.unwrap();
        assert_eq!(
            changes,
            vec![
                LockfileChange::UpdateRev(
                    "package1".into(),
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()
                ),
                LockfileChange::UpdateRev(
                    "package2".into(),
                    "cccccccccccccccccccccccccccccccccccccccc".into()
                )
            ]
        );
    }

    #[test]
    fn test_ls_remote_parsing() {
        let input = "\
ref: refs/heads/main    HEAD
59f5c322b48409c4d6d08cecae50b663151b22ed        HEAD
ref: refs/remotes/origin/main   refs/remotes/origin/HEAD
59f5c322b48409c4d6d08cecae50b663151b22ed        refs/remotes/origin/HEAD
";
        let lines = input
            .lines()
            .map(parse_git_ls_remote_line)
            .collect::<color_eyre::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            lines,
            vec![
                GitLsRemoteLine::SymRef {
                    target: "refs/heads/main".into(),
                    name: "HEAD".into()
                },
                GitLsRemoteLine::Branch {
                    rev: "59f5c322b48409c4d6d08cecae50b663151b22ed".into(),
                    target: "HEAD".into()
                },
                GitLsRemoteLine::SymRef {
                    target: "refs/remotes/origin/main".into(),
                    name: "refs/remotes/origin/HEAD".into()
                },
                GitLsRemoteLine::Branch {
                    rev: "59f5c322b48409c4d6d08cecae50b663151b22ed".into(),
                    target: "refs/remotes/origin/HEAD".into()
                }
            ]
        );
    }
}
