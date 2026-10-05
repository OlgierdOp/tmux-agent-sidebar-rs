//! macOS backend: libproc and `sysctl(KERN_PROCARGS2)` from libSystem.
//! No process spawns: each call is one system call.

use std::ffi::{c_int, c_uint, c_void};

const PROC_PPID_ONLY: u32 = 6;
const PROC_PIDVNODEPATHINFO: c_int = 9;
const CTL_KERN: c_int = 1;
const KERN_PROCARGS2: c_int = 49;
const MAXPATHLEN: usize = 1024;
/// size of `struct vnode_info` (vinfo_stat 136 + type 4 + pad 4 + fsid 8)
const VNODE_INFO_SIZE: usize = 152;
/// `struct proc_vnodepathinfo`: { cdir, rdir }, each { vnode_info, path[MAXPATHLEN] }
const VNODEPATHINFO_SIZE: usize = 2 * (VNODE_INFO_SIZE + MAXPATHLEN);

unsafe extern "C" {
    fn proc_listpids(kind: u32, typeinfo: u32, buffer: *mut c_void, buffersize: c_int) -> c_int;
    fn proc_name(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    fn proc_pidinfo(pid: c_int, flavor: c_int, arg: u64, buffer: *mut c_void, buffersize: c_int)
        -> c_int;
    fn sysctl(name: *mut c_int, namelen: c_uint, oldp: *mut c_void, oldlenp: *mut usize,
              newp: *mut c_void, newlen: usize) -> c_int;
}

fn c_str(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// argv of a process from `KERN_PROCARGS2`:
/// argc (i32), exec path, NUL padding, argv[0..argc], environment.
fn proc_args(pid: i64) -> Option<Vec<String>> {
    let mut mib = [CTL_KERN, KERN_PROCARGS2, pid as c_int];
    let mut buf = vec![0u8; 64 * 1024];
    let mut len = buf.len();
    let rc = unsafe {
        sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0)
    };
    if rc != 0 || len < 4 {
        return None;
    }
    let buf = &buf[..len];
    let argc = i32::from_ne_bytes(buf[..4].try_into().ok()?).max(0) as usize;
    let rest = &buf[4..];
    // skip the exec path and the NUL padding after it
    let path_end = rest.iter().position(|b| *b == 0)?;
    let start = rest[path_end..].iter().position(|b| *b != 0).map_or(rest.len(), |i| path_end + i);
    Some(
        rest[start..]
            .split(|b| *b == 0)
            .take(argc)
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect(),
    )
}

/// (comm, argv) of a process, or None if it is gone. argv is empty for
/// processes of other users (no permission).
pub fn proc_info(pid: i64) -> Option<(String, Vec<String>)> {
    let mut name = [0u8; 256];
    let n = unsafe { proc_name(pid as c_int, name.as_mut_ptr().cast(), name.len() as u32) };
    if n <= 0 {
        return None;
    }
    Some((c_str(&name[..n as usize]), proc_args(pid).unwrap_or_default()))
}

pub fn cwd(pid: i64) -> Option<String> {
    let mut info = vec![0u8; VNODEPATHINFO_SIZE];
    let n = unsafe {
        proc_pidinfo(pid as c_int, PROC_PIDVNODEPATHINFO, 0, info.as_mut_ptr().cast(),
                     info.len() as c_int)
    };
    if n as usize != VNODEPATHINFO_SIZE {
        return None;
    }
    let path = c_str(&info[VNODE_INFO_SIZE..VNODE_INFO_SIZE + MAXPATHLEN]);
    (!path.is_empty()).then_some(path)
}

/// The kernel lists the children of a pid directly: no snapshot needed.
pub struct Tree;

impl Tree {
    pub fn new() -> Self {
        Tree
    }

    pub fn children(&self, pid: i64) -> Vec<i64> {
        let mut pids = vec![0 as c_int; 256];
        loop {
            let size = (pids.len() * size_of::<c_int>()) as c_int;
            let bytes = unsafe {
                proc_listpids(PROC_PPID_ONLY, pid as u32, pids.as_mut_ptr().cast(), size)
            };
            if bytes <= 0 {
                return Vec::new();
            }
            // a full buffer may mean more children: try again with a bigger one
            if bytes < size || pids.len() >= 1 << 16 {
                let n = bytes as usize / size_of::<c_int>();
                return pids[..n].iter().filter(|p| **p > 0).map(|p| *p as i64).collect();
            }
            pids.resize(pids.len() * 4, 0);
        }
    }
}
