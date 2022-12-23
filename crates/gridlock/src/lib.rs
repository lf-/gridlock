//! The gridlock tool itself. This component manages lock files and the
//! updating thereof.

use std::{collections::BTreeMap, fs, io::Cursor, path::Path, process::Stdio};

use async_trait::async_trait;
use chrono::Utc;
use color_eyre::eyre::{eyre, Context};
use serde::{de::Visitor, Deserialize, Serialize, Serializer};

/// Lockfile format, loosely based on Niv's format, since it's simple and
/// mostly a good design.
#[derive(Deserialize, Serialize)]
pub struct Lockfile {
    pub packages: BTreeMap<String, Lock>,
    pub version: u16,
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
        branch_name: &str,
    ) -> color_eyre::Result<GitRevision>;

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

#[async_trait]
impl GitHubClient for OnlineGitHubClient {
    async fn branch_head(
        &self,
        owner: &str,
        repo: &str,
        branch_name: &str,
    ) -> color_eyre::Result<GitRevision> {
        // not confident this is the right approach/will not get us hosed by
        // rate limits
        let proc = tokio::process::Command::new("git")
            .arg("ls-remote")
            .arg(format!("https://github.com/{owner}/{repo}"))
            .arg(branch_name)
            .stdout(Stdio::piped())
            .output()
            .await?;
        let val = proc
            .stdout
            .splitn(2, |&v| v.is_ascii_whitespace())
            .next()
            .ok_or(eyre!("bad stdout"))?;
        Ok(String::from_utf8(val.to_vec())?)
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

        let mut hasher = nyarr::hash::NarHasher::new();
        nyarr::tar::tar_to_nar(Cursor::new(content), &mut hasher).map_err(|e| eyre!(e))?;

        Ok(Lock {
            owner: owner.into(),
            repo: repo.into(),
            branch: branch.into(),
            rev: rev.into(),
            url,
            last_updated: Some(UnixTimestamp(Utc::now())),
            sha256: hasher.digest(),
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
        let branch_head = client
            .branch_head(&lock.owner, &lock.repo, &lock.branch)
            .await
            .context("getting branch head")?;
        if branch_head != lock.rev {
            changes.push(LockfileChange::UpdateRev(name.to_string(), branch_head))
        }
    }

    Ok(changes)
}

pub fn read_lockfile(path: &Path) -> color_eyre::Result<Lockfile> {
    let content = fs::read(path).context("reading lockfile")?;
    let lockfile = serde_json::from_slice(&content)?;
    Ok(lockfile)
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
            branch_name: &str,
        ) -> color_eyre::Result<crate::GitRevision> {
            self.branch_maps
                .get(&(owner.to_string(), repo.to_string()))
                .ok_or_else(|| eyre!("unknown owner/repo {owner} {repo}"))?
                .get(branch_name)
                .ok_or_else(|| eyre!("unknown branch {branch_name}"))
                .cloned()
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
}
