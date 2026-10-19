use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::Duration,
};

#[derive(Clone, Debug)]
pub struct Resource {
    pub id: String,
    relative: PathBuf,
}
impl Resource {
    pub fn new(relative: PathBuf) -> Result<Self> {
        ensure!(
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
            "Media paths must be relative, without traversal"
        );
        let id = format!(
            "{:x}",
            Sha256::digest(relative.as_os_str().as_encoded_bytes())
        );
        Ok(Self { id, relative })
    }
}

/// Bootstrap descriptor. Treat the bearer token like a password; never log it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub port: u16,
    pub token: String,
}
impl Session {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && self.port != 0
                && self.token.len() == 32
                && self.token.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid streaming session"
        );
        Ok(())
    }
    pub fn catalog_url(&self) -> String {
        self.url("catalog")
    }
    pub fn media_url(&self, id: &str) -> Result<String> {
        ensure!(
            id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid media identifier"
        );
        Ok(self.url(&format!("media/{id}")))
    }
    fn url(&self, suffix: &str) -> String {
        format!(
            "http://127.0.0.1:{}/v1/{suffix}?token={}",
            self.port, self.token
        )
    }
}

pub struct Server {
    pub session: Session,
    stop: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
}
impl Server {
    /// Listen only on loopback. Native remote clients forward this port over SSH.
    /// Eight workers and 64 KiB copies bound memory, including for multi-GB films.
    pub fn start(
        root: &Path,
        resources: Vec<Resource>,
        mut catalog: serde_json::Value,
        token: String,
    ) -> Result<Self> {
        let root = root.canonicalize().context("Open media root")?;
        ensure!(root.is_dir(), "Media root must be a directory");
        let mut files = HashMap::new();
        for resource in resources {
            let path = root.join(&resource.relative).canonicalize()?;
            ensure!(
                path.starts_with(&root) && path.is_file(),
                "Media resource escapes the library"
            );
            ensure!(
                files.insert(resource.id, resource.relative).is_none(),
                "Duplicate media resource"
            );
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let session = Session {
            version: 1,
            port: listener.local_addr()?.port(),
            token,
        };
        session.validate()?;
        listener.set_nonblocking(true)?;
        if let Some(object) = catalog.as_object_mut() {
            object.insert(
                "streaming".into(),
                serde_json::json!({"hls":true,"version":1}),
            );
        }
        let data = Arc::new(Data {
            hls: super::hls::Hls::default(),
            root,
            files,
            catalog: serde_json::to_vec(&catalog)?,
            token: session.token.clone(),
        });
        ensure!(
            data.catalog.len() <= 16 * 1024 * 1024,
            "Catalog exceeds 16 MiB"
        );
        let stop = Arc::new(AtomicBool::new(false));
        let mut server = Self {
            session,
            stop,
            workers: Vec::new(),
        };
        for _ in 0..8 {
            let listener = listener.try_clone()?;
            let data = data.clone();
            let stop = server.stop.clone();
            server.workers.push(
                std::thread::Builder::new()
                    .name("star-media-http".into())
                    .spawn(move || {
                        while !stop.load(Ordering::Acquire) {
                            match listener.accept() {
                                Ok((mut socket, _)) => {
                                    // Darwin inherits the listener's nonblocking flag on accept.
                                    // Workers perform bounded blocking I/O; EWOULDBLOCK must
                                    // never truncate a movie when the send buffer fills.
                                    if socket.set_nonblocking(false).is_err() {
                                        continue;
                                    }
                                    let _ = socket.set_read_timeout(Some(Duration::from_secs(1)));
                                    let _ = socket.set_write_timeout(Some(Duration::from_secs(1)));
                                    let _ = data.request(&mut socket, &stop);
                                }
                                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                    std::thread::sleep(Duration::from_millis(10))
                                }
                                Err(_) => break,
                            }
                        }
                    })?,
            );
        }
        Ok(server)
    }
    pub fn address(&self) -> SocketAddr {
        (std::net::Ipv4Addr::LOCALHOST, self.session.port).into()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
struct Data {
    hls: super::hls::Hls,
    root: PathBuf,
    files: HashMap<String, PathBuf>,
    catalog: Vec<u8>,
    token: String,
}
impl Data {
    fn request(&self, stream: &mut TcpStream, stop: &AtomicBool) -> Result<()> {
        let mut headers = Vec::new();
        let began = std::time::Instant::now();
        while !headers.ends_with(b"\r\n\r\n") {
            if stop.load(Ordering::Acquire) || began.elapsed() > Duration::from_secs(3) {
                return Ok(());
            }
            let mut byte = [0];
            if stream.read(&mut byte)? == 0 {
                return Ok(());
            }
            headers.push(byte[0]);
            if headers.len() >= 16 * 1024 {
                return reply(
                    stream,
                    "431 Request Header Fields Too Large",
                    "text/plain",
                    &[],
                    false,
                );
            }
        }
        let headers = std::str::from_utf8(&headers)?;
        let mut lines = headers.split("\r\n");
        let request = lines
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>();
        if request.len() != 3 || !matches!(request[2], "HTTP/1.0" | "HTTP/1.1") {
            return reply(stream, "400 Bad Request", "text/plain", &[], false);
        }
        let head = request[0] == "HEAD";
        let (path, query) = request[1].split_once('?').unwrap_or((request[1], ""));
        let mut authorized = query
            .split('&')
            .any(|p| p.strip_prefix("token=") == Some(self.token.as_str()));
        let mut range = None;
        for line in lines.filter(|l| !l.is_empty()) {
            let Some((name, value)) = line.split_once(':') else {
                return reply(stream, "400 Bad Request", "text/plain", &[], head);
            };
            if name.eq_ignore_ascii_case("authorization") {
                authorized |= value.trim().strip_prefix("Bearer ") == Some(self.token.as_str());
            }
            if name.eq_ignore_ascii_case("range") {
                if range.is_some() {
                    return reply(stream, "400 Bad Request", "text/plain", &[], head);
                }
                range = Some(value.trim());
            }
        }
        if !authorized {
            return reply(stream, "401 Unauthorized", "text/plain", &[], head);
        }
        if !matches!(request[0], "GET" | "HEAD") {
            write!(stream, "HTTP/1.1 405 Method Not Allowed\r\nAllow: GET, HEAD\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
            return Ok(());
        }
        if path == "/v1/catalog" {
            return reply(stream, "200 OK", "application/json", &self.catalog, head);
        }
        if path == "/v1/clock" {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis();
            return reply(
                stream,
                "200 OK",
                "application/json",
                format!("{{\"unix_ms\":{now}}}").as_bytes(),
                head,
            );
        }
        if let Some(route) = path.strip_prefix("/v1/hls/") {
            return self.hls_request(stream, route, range, head, stop);
        }
        let Some(relative) = path
            .strip_prefix("/v1/media/")
            .and_then(|id| self.files.get(id))
        else {
            return reply(stream, "404 Not Found", "text/plain", &[], head);
        };
        let file = self
            .root
            .join(relative)
            .canonicalize()
            .ok()
            .filter(|p| p.starts_with(&self.root))
            .and_then(|p| File::open(p).ok());
        let Some(mut file) = file else {
            return reply(stream, "404 Not Found", "text/plain", &[], head);
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return reply(stream, "404 Not Found", "text/plain", &[], head);
        }
        let size = metadata.len();
        let (start, length, partial) = match range {
            None => (0, size, false),
            Some(value) => match byte_range(value, size) {
                Some((start, end)) => (start, end - start + 1, true),
                None => {
                    write!(stream, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{size}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
                    return Ok(());
                }
            },
        };
        let content_type = match relative
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "mp4" | "m4v" => "video/mp4",
            "mov" => "video/quicktime",
            "mkv" => "video/x-matroska",
            "webm" => "video/webm",
            "ts" | "mts" | "m2ts" => "video/mp2t",
            _ => "application/octet-stream",
        };
        write!(stream, "HTTP/1.1 {}\r\nContent-Type: {content_type}\r\nContent-Length: {length}\r\nAccept-Ranges: bytes\r\nCache-Control: no-store\r\nConnection: close\r\n", if partial { "206 Partial Content" } else { "200 OK" })?;
        if partial {
            write!(
                stream,
                "Content-Range: bytes {start}-{}/{size}\r\n",
                start + length - 1
            )?;
        }
        stream.write_all(b"\r\n")?;
        if !head {
            file.seek(SeekFrom::Start(start))?;
            let mut remaining = length;
            let mut buffer = [0; 64 * 1024];
            while remaining > 0 && !stop.load(Ordering::Acquire) {
                let limit = remaining.min(buffer.len() as u64) as usize;
                let n = file.read(&mut buffer[..limit])?;
                ensure!(n != 0, "Media changed during streaming");
                stream.write_all(&buffer[..n])?;
                remaining -= n as u64;
            }
        }
        Ok(())
    }
    fn hls_request(
        &self,
        stream: &mut TcpStream,
        route: &str,
        range: Option<&str>,
        head: bool,
        stop: &AtomicBool,
    ) -> Result<()> {
        let pieces = route.split('/').collect::<Vec<_>>();
        let valid = matches!(
            pieces.as_slice(),
            [_, "master.m3u8"] | [_, _, "index.m3u8"] | [_, _, _, "init.mp4" | "media.m4s"]
        );
        let Some(relative) = pieces
            .first()
            .filter(|_| valid)
            .and_then(|id| self.files.get(*id))
        else {
            return reply(stream, "404 Not Found", "text/plain", &[], head);
        };
        let source = self
            .root
            .join(relative)
            .canonicalize()
            .ok()
            .filter(|p| p.starts_with(&self.root) && p.is_file());
        let Some(source) = source else {
            return reply(stream, "404 Not Found", "text/plain", &[], head);
        };
        let prepared = (|| -> Result<_> {
            let movie = self.hls.movie(pieces[0], &source, stop)?;
            if pieces.len() == 2 {
                return Ok(HlsResponse::Playlist(movie.master(pieces[0], &self.token)));
            }
            if pieces.len() == 3 {
                return Ok(HlsResponse::Playlist(movie.playlist(
                    pieces[0],
                    pieces[1],
                    &self.token,
                )?));
            }
            let index = pieces[2].parse::<u32>().context("Invalid segment index")?;
            Ok(HlsResponse::Part(movie.part(
                pieces[1],
                index,
                pieces[3] == "init.mp4",
                stop,
            )?))
        })();
        match prepared {
            Ok(HlsResponse::Playlist(bytes)) => reply(
                stream,
                "200 OK",
                "application/vnd.apple.mpegurl",
                &bytes,
                head,
            ),
            Ok(HlsResponse::Part(part)) => send_part(stream, part, range, head, stop),
            Err(error) => {
                eprintln!("Adaptive movie request failed: {error}");
                reply(stream,"503 Service Unavailable","text/plain",b"The host could not prepare this movie stream. Check FFmpeg and hardware encoder support.",head)
            }
        }
    }
}
enum HlsResponse {
    Playlist(Vec<u8>),
    Part(super::hls::Part),
}
fn send_part(
    stream: &mut TcpStream,
    mut part: super::hls::Part,
    range: Option<&str>,
    head: bool,
    stop: &AtomicBool,
) -> Result<()> {
    let size = part.length;
    let (start, length, partial) = match range {
        None => (0, size, false),
        Some(value) => match byte_range(value, size) {
            Some((start, end)) => (start, end - start + 1, true),
            None => {
                write!(stream,"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{size}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
                return Ok(());
            }
        },
    };
    write!(stream,"HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {length}\r\nAccept-Ranges: bytes\r\nCache-Control: private, max-age=3600\r\nConnection: close\r\n",if partial{"206 Partial Content"}else{"200 OK"},part.mime)?;
    if partial {
        write!(
            stream,
            "Content-Range: bytes {start}-{}/{size}\r\n",
            start + length - 1
        )?;
    }
    stream.write_all(b"\r\n")?;
    if !head {
        part.file.seek(SeekFrom::Start(part.offset + start))?;
        let mut buffer = [0; 64 * 1024];
        let mut remaining = length;
        while remaining > 0 && !stop.load(Ordering::Acquire) {
            let limit = remaining.min(buffer.len() as u64) as usize;
            let n = part.file.read(&mut buffer[..limit])?;
            ensure!(n > 0, "Truncated HLS segment");
            stream.write_all(&buffer[..n])?;
            remaining -= n as u64;
        }
    }
    Ok(())
}
fn reply(stream: &mut TcpStream, status: &str, mime: &str, body: &[u8], head: bool) -> Result<()> {
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len())?;
    if !head {
        stream.write_all(body)?;
    }
    Ok(())
}
fn byte_range(value: &str, size: u64) -> Option<(u64, u64)> {
    if size == 0 {
        return None;
    }
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    if start.is_empty() {
        let count = end.parse::<u64>().ok()?.min(size);
        return (count > 0).then(|| (size - count, size - 1));
    }
    let start = start.parse::<u64>().ok()?;
    let end = if end.is_empty() {
        size - 1
    } else {
        end.parse::<u64>().ok()?.min(size - 1)
    };
    (start <= end && start < size).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(server: &Server, method: &str, path: &str, range: &str) -> Vec<u8> {
        let mut stream = TcpStream::connect(server.address()).unwrap();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\n{range}\r\n"
        )
        .unwrap();
        let mut bytes = vec![];
        stream.read_to_end(&mut bytes).unwrap();
        bytes
    }
    #[test]
    fn player_ranges_authentication_and_head_preserve_original_bytes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("movie.mp4"), b"0123456789").unwrap();
        let resource = Resource::new("movie.mp4".into()).unwrap();
        let id = resource.id.clone();
        let server = Server::start(
            root.path(),
            vec![resource],
            serde_json::json!({"movies":1}),
            "a".repeat(32),
        )
        .unwrap();
        let response = String::from_utf8(request(&server, "GET", "/v1/catalog", "")).unwrap();
        assert!(response.starts_with("HTTP/1.1 401"), "{response:?}");
        assert!(
            request(&server, "GET", &format!("/v1/hls/{id}/master.m3u8"), "")
                .starts_with(b"HTTP/1.1 401")
        );
        assert!(request(
            &server,
            "GET",
            &format!("/v1/hls/unknown/master.m3u8?token={}", server.session.token),
            ""
        )
        .starts_with(b"HTTP/1.1 404"));
        assert!(request(
            &server,
            "GET",
            &format!("/v1/hls/{id}/../secret?token={}", server.session.token),
            ""
        )
        .starts_with(b"HTTP/1.1 404"));
        let path = format!("/v1/media/{id}?token={}", server.session.token);
        let bytes = request(&server, "GET", &path, "Range: bytes=2-5\r\n");
        assert!(bytes.starts_with(b"HTTP/1.1 206"));
        assert!(bytes.ends_with(b"\r\n\r\n2345"));
        let bytes = request(&server, "GET", &path, "Range: bytes=-3\r\n");
        assert!(bytes.ends_with(b"789"));
        let bytes = request(&server, "HEAD", &path, "Range: bytes=2-\r\n");
        assert!(String::from_utf8_lossy(&bytes).contains("Content-Length: 8\r\n"));
        assert!(bytes.ends_with(b"\r\n\r\n"));
        assert!(request(&server, "GET", &path, "Range: bytes=99-\r\n").starts_with(b"HTTP/1.1 416"));
        assert!(
            request(&server, "GET", &path, "Range: bytes=0-1,4-5\r\n").starts_with(b"HTTP/1.1 416")
        );
        let bytes = request(&server, "GET", &path, "");
        assert!(bytes.ends_with(b"0123456789"));
        assert!(request(&server, "POST", &path, "").starts_with(b"HTTP/1.1 405"));
    }
    #[test]
    fn large_sources_are_not_truncated_when_socket_buffers_fill() {
        let root = tempfile::tempdir().unwrap();
        let bytes = (0..4 * 1024 * 1024)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>();
        std::fs::write(root.path().join("large.mp4"), &bytes).unwrap();
        let resource = Resource::new("large.mp4".into()).unwrap();
        let id = resource.id.clone();
        let server = Server::start(
            root.path(),
            vec![resource],
            serde_json::json!({}),
            "c".repeat(32),
        )
        .unwrap();
        let response = request(
            &server,
            "GET",
            &format!("/v1/media/{id}?token={}", server.session.token),
            "",
        );
        let start = response.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
        assert!(
            &response[start..] == bytes.as_slice(),
            "Large source bytes were truncated or changed"
        );
    }
    #[test]
    fn resources_never_authorize_traversal_or_external_symlinks() {
        assert!(Resource::new("../secret.mp4".into()).is_err());
        assert!(Resource::new("/secret.mp4".into()).is_err());
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), root.path().join("escape.mp4")).unwrap();
            assert!(Server::start(
                root.path(),
                vec![Resource::new("escape.mp4".into()).unwrap()],
                serde_json::json!({}),
                "b".repeat(32)
            )
            .is_err());
        }
    }
}
