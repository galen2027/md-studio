use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAGIC: &[u8; 16] = b"MDPREVIEW-IPC\x01\0\0";
const ACK: &[u8; 9] = b"MDP-ACK-1";
const PROTOCOL_VERSION: u8 = 1;
const MAX_FRAME_BYTES: usize = 64 * 1024;
const MAX_PATHS: usize = 128;
const PORT_BASE: u16 = 20_000;
const PORT_SPAN: u16 = 20_000;
const PORT_CANDIDATES: u16 = 12;
const PORT_STRIDE: u16 = 1_543;
const PROBE_TIMEOUT: Duration = Duration::from_millis(250);
const IO_TIMEOUT: Duration = Duration::from_millis(900);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstanceRequest {
    version: u8,
    instance_key: u64,
    pub paths: Vec<String>,
    pub edit: bool,
}

pub struct SingleInstanceServer {
    listener: TcpListener,
    instance_key: u64,
}

pub enum InstanceRole {
    Primary(SingleInstanceServer),
    Forwarded,
    Standalone,
}

enum ProbeResult {
    Accepted,
    NotOurInstance,
}

pub fn acquire(config_dir: &Path, paths: &[PathBuf], edit: bool) -> InstanceRole {
    let instance_key = stable_instance_key(config_dir);
    let requests = chunk_requests(instance_key, paths, edit);
    let first_request = &requests[0];

    for offset in 0..PORT_CANDIDATES {
        let addr = candidate_address(instance_key, offset);
        match TcpListener::bind(addr) {
            Ok(listener) => {
                return InstanceRole::Primary(SingleInstanceServer {
                    listener,
                    instance_key,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                // Bind-first keeps a normal cold start independent of socket
                // probe timeouts. If another process won the startup race, its
                // bound listener already queues this request until the server
                // thread starts.
                if matches!(
                    forward_to_address(addr, first_request),
                    Ok(ProbeResult::Accepted)
                ) {
                    forward_remaining_requests(addr, &requests[1..]);
                    return InstanceRole::Forwarded;
                }
            }
            Err(_) => {}
        }
    }

    // A local policy or unrelated process occupied every candidate. Opening
    // the document is more important than enforcing a singleton, so degrade
    // to an independent window instead of failing startup.
    InstanceRole::Standalone
}

fn chunk_requests(instance_key: u64, paths: &[PathBuf], edit: bool) -> Vec<InstanceRequest> {
    let mut requests = Vec::new();
    let mut current = InstanceRequest {
        version: PROTOCOL_VERSION,
        instance_key,
        paths: Vec::new(),
        edit,
    };

    for path in paths {
        let value = path.to_string_lossy().into_owned();
        current.paths.push(value);
        let too_many = current.paths.len() > MAX_PATHS;
        let too_large = serde_json::to_vec(&current)
            .map(|payload| payload.len() > MAX_FRAME_BYTES)
            .unwrap_or(true);
        if too_many || too_large {
            let last = current.paths.pop().expect("request path was just appended");
            if !current.paths.is_empty() {
                requests.push(current);
            }
            current = InstanceRequest {
                version: PROTOCOL_VERSION,
                instance_key,
                paths: vec![last],
                edit,
            };
        }
    }
    if !current.paths.is_empty() || requests.is_empty() {
        requests.push(current);
    }
    requests
}

fn forward_remaining_requests(addr: SocketAddr, requests: &[InstanceRequest]) {
    for request in requests {
        let mut accepted = false;
        for _ in 0..3 {
            if matches!(forward_to_address(addr, request), Ok(ProbeResult::Accepted)) {
                accepted = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        if !accepted {
            eprintln!(
                "MD Preview could not forward {} additional path(s) to the running instance",
                request.paths.len()
            );
        }
    }
}

impl SingleInstanceServer {
    pub fn start<F>(self, handler: F)
    where
        F: Fn(InstanceRequest) -> bool + Send + 'static,
    {
        std::thread::spawn(move || {
            for connection in self.listener.incoming() {
                let Ok(mut stream) = connection else {
                    continue;
                };
                let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
                let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                let Ok(request) = read_request(&mut stream) else {
                    continue;
                };
                if request.version != PROTOCOL_VERSION
                    || request.instance_key != self.instance_key
                    || request.paths.len() > MAX_PATHS
                {
                    continue;
                }
                if handler(request) {
                    let _ = stream.write_all(ACK);
                    let _ = stream.flush();
                }
            }
        });
    }
}

fn candidate_address(instance_key: u64, offset: u16) -> SocketAddr {
    let first = (instance_key % u64::from(PORT_SPAN)) as u16;
    // Spread candidates across the range instead of using consecutive ports.
    // Windows/Hyper-V commonly reserves contiguous blocks, which previously
    // made every candidate unavailable for some config-directory hashes.
    let port = PORT_BASE + ((first + offset * PORT_STRIDE) % PORT_SPAN);
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
}

fn forward_to_address(addr: SocketAddr, request: &InstanceRequest) -> io::Result<ProbeResult> {
    let mut stream = TcpStream::connect_timeout(&addr, PROBE_TIMEOUT)?;
    stream.set_read_timeout(Some(PROBE_TIMEOUT))?;
    stream.set_write_timeout(Some(PROBE_TIMEOUT))?;
    write_request(&mut stream, request)?;
    let mut ack = [0u8; ACK.len()];
    match stream.read_exact(&mut ack) {
        Ok(()) if &ack == ACK => Ok(ProbeResult::Accepted),
        Ok(()) => Ok(ProbeResult::NotOurInstance),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::TimedOut
                    | io::ErrorKind::WouldBlock
                    | io::ErrorKind::ConnectionReset
            ) =>
        {
            Ok(ProbeResult::NotOurInstance)
        }
        Err(error) => Err(error),
    }
}

fn write_request<W: Write>(writer: &mut W, request: &InstanceRequest) -> io::Result<()> {
    let payload = serde_json::to_vec(request).map_err(io::Error::other)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "single-instance request is too large",
        ));
    }
    writer.write_all(MAGIC)?;
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(&payload)
}

fn read_request<R: Read>(reader: &mut R) -> io::Result<InstanceRequest> {
    let mut magic = [0u8; MAGIC.len()];
    reader.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "single-instance protocol magic mismatch",
        ));
    }
    let mut length = [0u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "single-instance request length is invalid",
        ));
    }
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload).map_err(io::Error::other)
}

fn stable_instance_key(config_dir: &Path) -> u64 {
    // Do not canonicalize here. On Windows an existing path is commonly
    // returned as `\\?\C:\...`, while the same not-yet-created path stays
    // `C:\...`. That would change the instance key between first and second
    // launch on a clean machine. A lexical absolute identity is stable across
    // the directory's existence transition and is all this local port
    // namespace needs.
    let absolute = if config_dir.is_absolute() {
        config_dir.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(config_dir)
    };
    let identity = normalize_windows_path_identity(&absolute);

    // Stable FNV-1a instead of DefaultHasher, whose output is not a persistent
    // cross-version contract.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in identity.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn normalize_windows_path_identity(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('/', "\\");
    let without_verbatim = raw
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| raw.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or(raw);
    without_verbatim.trim_end_matches('\\').to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs;
    use std::io::Cursor;
    use std::sync::mpsc;
    use std::time::Instant;

    #[test]
    fn request_codec_round_trips_unicode_paths_and_edit_mode() {
        let request = InstanceRequest {
            version: PROTOCOL_VERSION,
            instance_key: 42,
            paths: vec![
                r"C:\资料 目录\方案.md".to_string(),
                r"D:\第二份.md".to_string(),
            ],
            edit: true,
        };
        let mut bytes = Vec::new();

        write_request(&mut bytes, &request).unwrap();
        let decoded = read_request(&mut Cursor::new(bytes)).unwrap();

        assert_eq!(decoded, request);
    }

    #[test]
    fn request_codec_rejects_bad_magic_and_oversized_frames() {
        let mut bad_magic = vec![0u8; MAGIC.len() + 4];
        assert_eq!(
            read_request(&mut Cursor::new(&mut bad_magic))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );

        let mut oversized = Vec::from(MAGIC.as_slice());
        oversized.extend_from_slice(&((MAX_FRAME_BYTES as u32) + 1).to_le_bytes());
        assert_eq!(
            read_request(&mut Cursor::new(oversized))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn loopback_forwarding_waits_for_an_exact_ack() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream).unwrap();
            tx.send(request).unwrap();
            stream.write_all(ACK).unwrap();
        });
        let request = InstanceRequest {
            version: PROTOCOL_VERSION,
            instance_key: 7,
            paths: vec![r"C:\中文 空格.md".to_string()],
            edit: false,
        };

        assert!(matches!(
            forward_to_address(addr, &request).unwrap(),
            ProbeResult::Accepted
        ));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), request);
    }

    #[test]
    fn instance_key_and_candidate_port_are_stable() {
        let path = Path::new(r"C:\Users\Test\AppData\Local\md-preview");
        let first = stable_instance_key(path);
        let second = stable_instance_key(path);

        assert_eq!(first, second);
        assert_eq!(candidate_address(first, 0), candidate_address(second, 0));
        assert_ne!(candidate_address(first, 0), candidate_address(first, 1));
    }

    #[test]
    fn candidate_ports_avoid_the_regressed_windows_reserved_block() {
        // Regression key whose old consecutive candidates were all inside one
        // Windows-reserved port block (56009..56020).
        let key = 17_810_718_278_518_454_009u64;
        let ports = (0..PORT_CANDIDATES)
            .map(|offset| candidate_address(key, offset).port())
            .collect::<Vec<_>>();

        assert_eq!(ports[0], 34_009);
        assert!(ports.iter().all(|port| (20_000..40_000).contains(port)));
        assert_eq!(
            ports.iter().copied().collect::<HashSet<_>>().len(),
            ports.len()
        );
        assert!(ports.windows(2).all(|pair| pair[0].abs_diff(pair[1]) > 1));
    }

    #[test]
    fn free_candidate_acquires_primary_without_probe_delay() {
        let root = std::env::temp_dir().join(format!(
            "md-preview-fast-acquire-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = (0..256)
            .map(|suffix| root.join(suffix.to_string()))
            .find(|candidate| {
                let addr = candidate_address(stable_instance_key(candidate), 0);
                TcpListener::bind(addr).map(drop).is_ok()
            })
            .expect("test needs one free loopback candidate");

        let started = Instant::now();
        let role = acquire(&config, &[], false);

        assert!(matches!(role, InstanceRole::Primary(_)));
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "a free first candidate must not pay a network probe timeout"
        );
    }

    #[test]
    fn instance_key_does_not_change_when_config_directory_is_created() {
        let root = std::env::temp_dir().join(format!(
            "md-preview-instance-key-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = root.join("new-config");
        let before = stable_instance_key(&config);

        fs::create_dir_all(&config).unwrap();
        let after = stable_instance_key(&config);

        assert_eq!(before, after);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn large_path_lists_are_chunked_without_dropping_documents() {
        let paths = (0..300)
            .map(|index| PathBuf::from(format!(r"C:\资料\文档 {index}.md")))
            .collect::<Vec<_>>();

        let requests = chunk_requests(99, &paths, true);

        assert!(requests.len() >= 3);
        assert!(requests
            .iter()
            .all(|request| request.paths.len() <= MAX_PATHS));
        assert!(requests
            .iter()
            .all(|request| { serde_json::to_vec(request).unwrap().len() <= MAX_FRAME_BYTES }));
        assert_eq!(
            requests
                .iter()
                .flat_map(|request| request.paths.iter())
                .count(),
            paths.len()
        );
        assert!(requests.iter().all(|request| request.edit));
    }
}
