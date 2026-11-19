//! Starts a run's process on its PTY with `posix_spawn` instead of a fork (Linux, glibc).
//!
//! portable-pty spawns through `std::process::Command` with a `pre_exec` (reset signals, `setsid`,
//! `TIOCSCTTY`, close stray fds), and any `pre_exec` makes std fork. Forking a large
//! multi-threaded process copies its page tables while holding its memory lock, so every other
//! thread's page faults wait (the test binary: 1.3 GB, a thread per CPU, hundreds of shells).
//! glibc's `posix_spawn` (`clone(CLONE_VM | CLONE_VFORK)`) does the same work without the copy:
//! `POSIX_SPAWN_SETSID` makes the child a session leader before the file actions run, and opening
//! the slave by its path (no `O_NOCTTY`) as a session leader without a terminal makes it the
//! controlling terminal, as `TIOCSCTTY` did. The rest matches portable-pty: the program found on
//! the child's `PATH` relative to `cwd`, `argv[0]` as given, an empty signal mask, default
//! dispositions, fds above 2 closed.

use std::ffi::{CString, OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};

/// Signals whose disposition the child gets back as default: portable-pty's list, plus SIGPIPE
/// (Rust's runtime ignores it, and an ignored signal survives exec).
const DEFAULT_SIGNALS: [libc::c_int; 7] = [
    libc::SIGCHLD,
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTERM,
    libc::SIGALRM,
    libc::SIGPIPE,
];

/// Blocking: spawns `argv` in `cwd` with exactly `env`, its stdin / stdout / stderr the terminal
/// at `tty`, which becomes its controlling terminal. Returns the pid (also its session and
/// process group id).
pub(super) fn spawn(argv: &[OsString], cwd: &Path, env: &[(OsString, OsString)], tty: &Path) -> io::Result<i32> {
    let program = find_program(&argv[0], cwd, env)?;
    let program = cstring(program.as_os_str())?;
    let tty = cstring(tty.as_os_str())?;
    let cwd_c = cstring(cwd.as_os_str())?;
    let args = argv.iter().map(|a| cstring(a)).collect::<io::Result<Vec<_>>>()?;
    let vars = env
        .iter()
        .map(|(k, v)| {
            let mut kv = k.clone();
            kv.push("=");
            kv.push(v);
            cstring(&kv)
        })
        .collect::<io::Result<Vec<_>>>()?;
    let mut arg_ptrs: Vec<*mut libc::c_char> = args.iter().map(|a| a.as_ptr() as *mut _).collect();
    arg_ptrs.push(std::ptr::null_mut());
    let mut env_ptrs: Vec<*mut libc::c_char> = vars.iter().map(|v| v.as_ptr() as *mut _).collect();
    env_ptrs.push(std::ptr::null_mut());

    // SAFETY: the file actions and attributes are initialised before use and destroyed by the
    // guards; every pointer handed to glibc points into a CString or Vec that outlives the call.
    unsafe {
        let mut actions = Actions(std::mem::zeroed());
        check(libc::posix_spawn_file_actions_init(&mut actions.0))?;
        check(libc::posix_spawn_file_actions_addopen(
            &mut actions.0,
            0,
            tty.as_ptr(),
            libc::O_RDWR,
            0,
        ))?;
        check(libc::posix_spawn_file_actions_adddup2(&mut actions.0, 0, 1))?;
        check(libc::posix_spawn_file_actions_adddup2(&mut actions.0, 0, 2))?;
        check(libc::posix_spawn_file_actions_addchdir_np(
            &mut actions.0,
            cwd_c.as_ptr(),
        ))?;
        // glibc 2.34+; without it the app's fds still don't leak (Rust opens them close-on-exec).
        if let Some(closefrom) = addclosefrom() {
            check(closefrom(&mut actions.0, 3))?;
        }

        let mut attr = Attr(std::mem::zeroed());
        check(libc::posix_spawnattr_init(&mut attr.0))?;
        let mut mask: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut mask);
        check(libc::posix_spawnattr_setsigmask(&mut attr.0, &mask))?;
        let mut defaults: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut defaults);
        for signal in DEFAULT_SIGNALS {
            libc::sigaddset(&mut defaults, signal);
        }
        check(libc::posix_spawnattr_setsigdefault(&mut attr.0, &defaults))?;
        let flags =
            libc::POSIX_SPAWN_SETSID as libc::c_int | libc::POSIX_SPAWN_SETSIGMASK | libc::POSIX_SPAWN_SETSIGDEF;
        check(libc::posix_spawnattr_setflags(&mut attr.0, flags as libc::c_short))?;

        let mut pid: libc::pid_t = 0;
        check(libc::posix_spawn(
            &mut pid,
            program.as_ptr(),
            &actions.0,
            &attr.0,
            arg_ptrs.as_ptr(),
            env_ptrs.as_ptr(),
        ))?;
        Ok(pid)
    }
}

/// Blocking: reaps `pid`, as portable-pty reports a status (a signal's `strsignal` name, code 1).
pub(super) fn reap(pid: i32) -> io::Result<portable_pty::ExitStatus> {
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: waitpid writes the status into `status`.
        let r = unsafe { libc::waitpid(pid, &mut status, 0) };
        if r == pid {
            return Ok(std::process::ExitStatus::from_raw(status).into());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// `exe` as portable-pty resolves it: a name with a slash relative to `cwd`, any other relative
/// name searched on the child's `PATH` (relative entries against `cwd`).
fn find_program(exe: &OsStr, cwd: &Path, env: &[(OsString, OsString)]) -> io::Result<PathBuf> {
    let path = Path::new(exe);
    let executable = |p: &Path| {
        p.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if path.is_absolute() || exe.as_bytes().contains(&b'/') {
        let full = cwd.join(path);
        return if executable(&full) {
            Ok(full)
        } else if full.exists() {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is not executable", full.display()),
            ))
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} doesn't exist", full.display()),
            ))
        };
    }
    let search = env
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.as_os_str())
        .unwrap_or_default();
    std::env::split_paths(search)
        .map(|dir| cwd.join(dir).join(path))
        .find(|p| executable(p))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{} isn't on the PATH", path.display())))
}

fn cstring(s: &OsStr) -> io::Result<CString> {
    CString::new(s.as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a NUL byte in an argument"))
}

/// posix_spawn's functions return the error number instead of setting errno.
fn check(r: libc::c_int) -> io::Result<()> {
    if r == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(r))
    }
}

type AddClosefrom = unsafe extern "C" fn(*mut libc::posix_spawn_file_actions_t, libc::c_int) -> libc::c_int;

/// glibc's `posix_spawn_file_actions_addclosefrom_np`, looked up so an older glibc still runs.
fn addclosefrom() -> Option<AddClosefrom> {
    static FOUND: std::sync::OnceLock<Option<AddClosefrom>> = std::sync::OnceLock::new();
    *FOUND.get_or_init(|| {
        // SAFETY: dlsym with a NUL-terminated name; a non-null result is that function, whose
        // signature AddClosefrom matches.
        let f = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"posix_spawn_file_actions_addclosefrom_np".as_ptr()) };
        (!f.is_null()).then(|| unsafe { std::mem::transmute::<*mut libc::c_void, AddClosefrom>(f) })
    })
}

struct Actions(libc::posix_spawn_file_actions_t);

impl Drop for Actions {
    fn drop(&mut self) {
        // SAFETY: destroying file actions is harmless even if init failed (zeroed).
        unsafe { libc::posix_spawn_file_actions_destroy(&mut self.0) };
    }
}

struct Attr(libc::posix_spawnattr_t);

impl Drop for Attr {
    fn drop(&mut self) {
        // SAFETY: as for Actions.
        unsafe { libc::posix_spawnattr_destroy(&mut self.0) };
    }
}
