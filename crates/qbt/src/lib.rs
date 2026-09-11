//! qBittorrent `WebAPI` client.
//!
//! Blocking on purpose: the import path is a short sequence of calls, and
//! keeping Tokio out of this crate matches the rest of the workspace. The
//! [`Client`] trait is what `tsync-import` depends on, so tests can fake a
//! destination without opening a socket.

use std::io::Read;

use serde::Deserialize;

/// Bytes/sec applied for the duration of an import. `0` means unlimited in
/// qBittorrent, so this must never be zero.
pub const CIRCUIT_BREAKER_BPS: i64 = 1;

/// Why a `WebAPI` call failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// HTTP or transport failure.
    #[error("qBittorrent API: {0}")]
    Http(String),
    /// Login was rejected.
    #[error("qBittorrent login failed")]
    LoginDenied,
    /// The server returned a body we could not use.
    #[error("qBittorrent API: unexpected response: {0}")]
    Unexpected(String),
}

impl From<ureq::Error> for Error {
    fn from(error: ureq::Error) -> Self {
        Self::Http(error.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Http(error.to_string())
    }
}

/// One torrent as `torrents/info` reports it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Torrent {
    /// Infohash, lower-case hex.
    pub hash: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Client state string (`stalledUP`, `missingFiles`, …).
    pub state: String,
    /// 0.0–1.0 as the client reports it.
    #[serde(default)]
    pub progress: f64,
    /// Bytes still missing, as the client reports it.
    #[serde(default)]
    pub amount_left: u64,
    /// Save path, if present.
    #[serde(default)]
    pub save_path: String,
}

/// Whether `state` is an announcing seed. Used by the dual-seed guard.
#[must_use]
pub fn is_seeding(state: &str) -> bool {
    matches!(state, "uploading" | "stalledUP" | "forcedUP" | "queuedUP")
}

/// Whether the client is hashing pieces. Recheck after import looks like this.
#[must_use]
pub fn is_checking(state: &str) -> bool {
    matches!(state, "checkingUP" | "checkingDL" | "checkingResumeData")
}

/// Whether the client is pulling from the swarm. Dest must not do this.
#[must_use]
pub fn is_downloading(state: &str) -> bool {
    matches!(
        state,
        "downloading" | "stalledDL" | "forcedDL" | "queuedDL" | "metaDL" | "allocating"
    )
}

/// Whether the torrent is paused. qBittorrent 5 renamed `paused*` to `stopped*`.
#[must_use]
pub fn is_stopped(state: &str) -> bool {
    matches!(state, "stoppedUP" | "stoppedDL" | "pausedUP" | "pausedDL")
}

/// Whether the client reports every piece present.
#[must_use]
pub fn is_piece_complete(torrent: &Torrent) -> bool {
    torrent.amount_left == 0 && torrent.progress >= 1.0
}

/// Operations import needs. Implemented by [`Session`] and by test fakes.
pub trait Client {
    /// Every torrent currently in the session.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the API call fails.
    fn list(&self) -> Result<Vec<Torrent>, Error>;

    /// Add a torrent file, paused, with a stop-after-check condition.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the API call fails or the client rejects the add.
    fn add_paused(&self, torrent: &[u8], filename: &str, save_path: &str) -> Result<(), Error>;

    /// Stop (qBittorrent 5) or pause (older) a torrent.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the API call fails.
    fn stop(&self, hash: &str) -> Result<(), Error>;

    /// Force a recheck. Hashing does not need the network.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the API call fails.
    fn recheck(&self, hash: &str) -> Result<(), Error>;

    /// Current global download limit in bytes/sec. `0` means unlimited.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the API call fails.
    fn download_limit(&self) -> Result<i64, Error>;

    /// Set the global download limit. `0` is unlimited.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the API call fails.
    fn set_download_limit(&self, bytes_per_sec: i64) -> Result<(), Error>;
}

impl<T: Client + ?Sized> Client for &T {
    fn list(&self) -> Result<Vec<Torrent>, Error> {
        (*self).list()
    }
    fn add_paused(&self, torrent: &[u8], filename: &str, save_path: &str) -> Result<(), Error> {
        (*self).add_paused(torrent, filename, save_path)
    }
    fn stop(&self, hash: &str) -> Result<(), Error> {
        (*self).stop(hash)
    }
    fn recheck(&self, hash: &str) -> Result<(), Error> {
        (*self).recheck(hash)
    }
    fn download_limit(&self) -> Result<i64, Error> {
        (*self).download_limit()
    }
    fn set_download_limit(&self, bytes_per_sec: i64) -> Result<(), Error> {
        (*self).set_download_limit(bytes_per_sec)
    }
}

/// Infohashes the client is currently seeding.
///
/// # Errors
///
/// Returns [`Error`] when listing fails.
pub fn seeding_hashes(client: &dyn Client) -> Result<Vec<String>, Error> {
    let mut hashes: Vec<String> = client
        .list()?
        .into_iter()
        .filter(|torrent| is_seeding(&torrent.state))
        .map(|torrent| torrent.hash.to_ascii_lowercase())
        .collect();
    hashes.sort();
    Ok(hashes)
}

/// A logged-in `WebAPI` session.
#[derive(Debug)]
pub struct Session {
    base: String,
    sid: String,
    /// qBittorrent 5 renamed pause → stop. Detected at login.
    stop_path: &'static str,
}

impl Session {
    /// Log in and discover which stop/pause endpoint this server speaks.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LoginDenied`] when credentials are rejected.
    pub fn login(base: &str, username: &str, password: &str) -> Result<Self, Error> {
        let base = base.trim_end_matches('/').to_owned();
        let response = ureq::post(&format!("{base}/api/v2/auth/login"))
            .send_form(&[("username", username), ("password", password)])?;

        let sid = cookie_sid(response.header("set-cookie")).ok_or(Error::LoginDenied)?;
        let status = response.status();
        let mut body = String::new();
        response.into_reader().read_to_string(&mut body)?;
        // 5.2 answers 204 + empty body + QBT_SID_<port>. Older: 200 + "Ok." + SID.
        if status != 204 && body.trim() != "Ok." {
            return Err(Error::LoginDenied);
        }

        let session = Self {
            base,
            sid,
            stop_path: "/api/v2/torrents/stop",
        };
        let stop_path = match session.webapi_version() {
            Ok(version) if uses_stop_endpoint(&version) => "/api/v2/torrents/stop",
            Ok(_) => "/api/v2/torrents/pause",
            Err(_) => "/api/v2/torrents/stop",
        };
        Ok(Self {
            stop_path,
            ..session
        })
    }

    fn webapi_version(&self) -> Result<String, Error> {
        Ok(self
            .get_text("/api/v2/app/webapiVersion")?
            .trim()
            .to_owned())
    }

    fn get_text(&self, path: &str) -> Result<String, Error> {
        let mut body = String::new();
        self.get(path)?.into_reader().read_to_string(&mut body)?;
        Ok(body)
    }

    fn get(&self, path: &str) -> Result<ureq::Response, Error> {
        Ok(ureq::get(&format!("{}{path}", self.base))
            .set("Cookie", &self.sid)
            .call()?)
    }

    fn post_form(&self, path: &str, fields: &[(&str, &str)]) -> Result<String, Error> {
        let mut body = String::new();
        ureq::post(&format!("{}{path}", self.base))
            .set("Cookie", &self.sid)
            .send_form(fields)?
            .into_reader()
            .read_to_string(&mut body)?;
        Ok(body)
    }
}

impl Client for Session {
    fn list(&self) -> Result<Vec<Torrent>, Error> {
        let body = self.get_text("/api/v2/torrents/info")?;
        let torrents: Vec<Torrent> =
            serde_json::from_str(&body).map_err(|error| Error::Unexpected(error.to_string()))?;
        Ok(torrents)
    }

    fn add_paused(&self, torrent: &[u8], filename: &str, save_path: &str) -> Result<(), Error> {
        let (content_type, body) = multipart_add(torrent, filename, save_path);
        let response = ureq::post(&format!("{}/api/v2/torrents/add", self.base))
            .set("Cookie", &self.sid)
            .set("Content-Type", &content_type)
            .send_bytes(&body)?;
        let status = response.status();
        let mut text = String::new();
        response.into_reader().read_to_string(&mut text)?;
        if status == 204 || text.trim() == "Ok." {
            Ok(())
        } else {
            Err(Error::Unexpected(text))
        }
    }

    fn stop(&self, hash: &str) -> Result<(), Error> {
        self.post_form(self.stop_path, &[("hashes", hash)])?;
        Ok(())
    }

    fn recheck(&self, hash: &str) -> Result<(), Error> {
        self.post_form("/api/v2/torrents/recheck", &[("hashes", hash)])?;
        Ok(())
    }

    fn download_limit(&self) -> Result<i64, Error> {
        let body = self.get_text("/api/v2/transfer/downloadLimit")?;
        body.trim().parse().map_err(|_| Error::Unexpected(body))
    }

    fn set_download_limit(&self, bytes_per_sec: i64) -> Result<(), Error> {
        let value = bytes_per_sec.to_string();
        self.post_form("/api/v2/transfer/setDownloadLimit", &[("limit", &value)])?;
        Ok(())
    }
}

/// qBittorrent 5 / `WebAPI` 2.11 renamed `pause`/`resume` to `stop`/`start`.
#[must_use]
pub fn uses_stop_endpoint(webapi_version: &str) -> bool {
    let mut parts = webapi_version.trim().split('.');
    let major: u32 = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    let minor: u32 = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    major > 2 || (major == 2 && minor >= 11)
}

fn cookie_sid(header: Option<&str>) -> Option<String> {
    let header = header?;
    header.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        let known = name == "SID" || name.starts_with("QBT_SID_");
        (known && !value.is_empty()).then(|| format!("{name}={value}"))
    })
}

fn multipart_add(torrent: &[u8], filename: &str, save_path: &str) -> (String, Vec<u8>) {
    let boundary = "----tsync-qbt";
    let mut body = Vec::new();
    for (name, value) in [
        ("savepath", save_path),
        ("paused", "true"),
        ("autoTMM", "false"),
        ("skip_checking", "false"),
        ("stopCondition", "FilesChecked"),
    ] {
        push_text_part(&mut body, boundary, name, value);
    }
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"torrents\"; filename=\"{filename}\"\r\n\
             Content-Type: application/x-bittorrent\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(torrent);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn push_text_part(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes(),
    );
}

#[cfg(test)]
mod cookie_tests {
    use super::{Torrent, cookie_sid};

    #[test]
    fn reads_sid_from_set_cookie() {
        assert_eq!(
            cookie_sid(Some("SID=abc123; HttpOnly; Path=/")),
            Some("SID=abc123".into())
        );
        assert_eq!(
            cookie_sid(Some("QBT_SID_8080=test-sid; HttpOnly; SameSite=Strict")),
            Some("QBT_SID_8080=test-sid".into())
        );
    }

    #[test]
    fn rejects_empty_sid() {
        assert_eq!(cookie_sid(Some("SID=; Path=/")), None);
        assert_eq!(cookie_sid(Some("QBT_SID_8080=; Path=/")), None);
    }

    #[test]
    fn stop_arrived_at_webapi_2_11() {
        assert!(!super::uses_stop_endpoint("2.9.3"));
        assert!(super::uses_stop_endpoint("2.11.2"));
        assert!(super::uses_stop_endpoint("2.11.0"));
    }

    #[test]
    fn seeding_states() {
        assert!(super::is_seeding("stalledUP"));
        assert!(super::is_seeding("forcedUP"));
        assert!(!super::is_seeding("stoppedUP"));
        assert!(!super::is_seeding("missingFiles"));
        assert!(!super::is_seeding("pausedUP"));
    }

    #[test]
    fn verify_state_helpers() {
        assert!(super::is_checking("checkingUP"));
        assert!(super::is_checking("checkingResumeData"));
        assert!(!super::is_checking("stoppedUP"));

        assert!(super::is_downloading("stalledDL"));
        assert!(super::is_downloading("forcedDL"));
        assert!(!super::is_downloading("checkingDL"));
        assert!(!super::is_downloading("stoppedDL"));

        assert!(super::is_stopped("stoppedUP"));
        assert!(super::is_stopped("pausedUP"));
        assert!(!super::is_stopped("stalledUP"));

        let complete = Torrent {
            hash: "aa".into(),
            name: "x".into(),
            state: "stoppedUP".into(),
            progress: 1.0,
            amount_left: 0,
            save_path: "/data".into(),
        };
        let incomplete = Torrent {
            amount_left: 512,
            progress: 0.5,
            ..complete.clone()
        };
        assert!(super::is_piece_complete(&complete));
        assert!(!super::is_piece_complete(&incomplete));
    }
}
