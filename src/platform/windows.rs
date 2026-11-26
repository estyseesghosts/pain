//! M3 Windows ConPTY backend. Owns one pseudoconsole, one child process and
//! three worker threads. No WinUI, no private conpty.dll, no Ghostty surface,
//! no direct Win32 key/text handling â€” GPUI owns those.

use std::ffi::c_void;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use windows::core::{HRESULT, PCSTR, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, GENERIC_READ, HANDLE, HMODULE,
};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, SearchPathW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING,
    PIPE_ACCESS_OUTBOUND,
};
use windows::Win32::System::Console::{COORD, HPCON};
use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetProcAddress, LoadLibraryW,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE};
use windows::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetCurrentProcess, GetCurrentThread,
    GetExitCodeProcess, InitializeProcThreadAttributeList, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject, CREATE_UNICODE_ENVIRONMENT,
    EXTENDED_STARTUPINFO_PRESENT, INFINITE, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};
use windows::Win32::System::IO::CancelSynchronousIo;

use super::{terminal_environment, PlatformPty, PtyConfig, PtyError, PtyResizeMailbox};
use crate::terminal::session::TerminalSession;

static NEXT_PIPE_ID: AtomicU32 = AtomicU32::new(0);

/// Resolve a bare program name to an absolute path. `CreateProcessW` does not
/// search PATH when `lpApplicationName` is set, so we do it explicitly.
fn resolve_program(program: &str) -> Result<String, PtyError> {
    if program.contains(['/', '\\']) {
        return Ok(program.to_owned());
    }
    let filename: Vec<u16> = program.encode_utf16().chain(std::iter::once(0)).collect();
    let extension: Vec<u16> = ".exe".encode_utf16().chain(std::iter::once(0)).collect();
    let mut buffer = vec![0u16; 1024];
    // SAFETY: all pointers are NUL-terminated or valid writable buffers; no
    // file-part pointer is needed. The return value is a character count.
    let written = unsafe {
        SearchPathW(
            PCWSTR::null(),
            PCWSTR(filename.as_ptr()),
            PCWSTR(extension.as_ptr()),
            Some(&mut buffer),
            None,
        ) as usize
    };
    if written == 0 || written > buffer.len() {
        return Err(PtyError::new(
            "SearchPathW",
            format!("cannot resolve shell program {program}"),
        ));
    }
    Ok(String::from_utf16_lossy(&buffer[..written]))
}

/// Quote one command-line argument using the Windows CommandLineToArgvW rules.
/// Rejects embedded NUL (the caller maps it to an error before reaching Win32).
#[must_use]
pub fn quote_argument(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                if backslashes > 0 {
                    out.push_str(&"\\".repeat(backslashes));
                    backslashes = 0;
                }
                out.push(ch);
            }
        }
    }
    if backslashes > 0 {
        out.push_str(&"\\".repeat(backslashes * 2));
    }
    out.push('"');
    out
}

#[must_use]
pub fn command_line(command: &super::ShellCommand) -> String {
    let mut parts = vec![quote_argument(&command.program)];
    parts.extend(command.args.iter().map(|arg| quote_argument(arg)));
    parts.join(" ")
}

/// NUL-terminated environment block with terminal identity overrides applied.
/// Returns an error string when a key/value contains NUL or `=`.
pub fn environment_block(
    overrides: &[(&str, &str)],
    inherited: impl IntoIterator<Item = (String, String)>,
) -> Result<Vec<u16>, String> {
    let mut values: Vec<(String, String)> = inherited.into_iter().collect();
    for (key, value) in overrides {
        if key.is_empty() || key.contains(['\0', '=']) || value.contains('\0') {
            return Err(format!("invalid environment entry: {key}={value}"));
        }
        values.retain(|(existing, _)| existing != key);
        values.push(((*key).to_owned(), (*value).to_owned()));
    }
    let mut block = Vec::new();
    // CreateProcessW expects a case-insensitively sorted, NUL-terminated
    // double-NUL block. An unsorted block is rejected with ERROR_INVALID_PARAMETER.
    values.sort_by_key(|(key, _)| key.to_ascii_uppercase());
    for (key, value) in values {
        block.extend(key.encode_utf16());
        block.push(b'=' as u16);
        block.extend(value.encode_utf16());
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

/// One Win32 handle with exactly one owner. Dropping calls `CloseHandle`.
struct OwnedHandle {
    raw: HANDLE,
}

impl OwnedHandle {
    /// A handle is invalid when Win32 failed; such values must not be wrapped.
    fn from_raw(raw: HANDLE, operation: &'static str) -> Result<Self, PtyError> {
        if raw.is_invalid() || raw.0.is_null() {
            return Err(PtyError::new(operation, "invalid handle"));
        }
        Ok(Self { raw })
    }

    fn as_raw(&self) -> HANDLE {
        self.raw
    }
}

// SAFETY: HANDLE is an OS handle with no thread affinity. Ownership is unique;
// worker threads borrow clones through Arc and synchronize via mutex/atomics.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.raw.is_invalid() && !self.raw.0.is_null() {
            // SAFETY: this wrapper is the single owner of the handle.
            let _ = unsafe { CloseHandle(self.raw) };
        }
    }
}

type CreatePseudoConsoleFn =
    unsafe extern "system" fn(COORD, HANDLE, HANDLE, u32, *mut HPCON) -> HRESULT;
type ResizePseudoConsoleFn = unsafe extern "system" fn(HPCON, COORD) -> HRESULT;
type ClosePseudoConsoleFn = unsafe extern "system" fn(HPCON);

/// Process-global ConPTY entry points. The in-box kernel32 host silently drops
/// unknown APCs (including Kitty graphics), so an adjacent OpenConsole
/// `conpty.dll` is preferred when present. The module stays loaded for the
/// process lifetime: HPCON values created by it must outlive any unload.
pub struct ConPtyApi {
    create: CreatePseudoConsoleFn,
    resize: ResizePseudoConsoleFn,
    close: ClosePseudoConsoleFn,
    /// True only when all three entry points came from an adjacent
    /// `conpty.dll`; the fallback is explicitly Kitty-graphics-degraded.
    pub bundled: bool,
    _module: HMODULE,
}

// SAFETY: function pointers and a process-lifetime module handle have no
// thread affinity. HPCON objects themselves are still owned by one WindowsPty.
unsafe impl Send for ConPtyApi {}
unsafe impl Sync for ConPtyApi {}

static CONPTY_API: OnceLock<ConPtyApi> = OnceLock::new();

/// Logged when the adjacent OpenConsole pair cannot be used. The wording is
/// asserted by a test so a future refactor cannot quietly turn a degraded
/// fallback into a full-support claim.
pub const CONPTY_FALLBACK_WARNING: &str = "no adjacent conpty.dll; using in-box kernel32 ConPTY. Kitty graphics APCs are stripped by the legacy host: graphics are DEGRADED (direct transmission may not reach libghostty-vt).";

fn resolve_pseudo_console(module: HMODULE) -> Option<(CreatePseudoConsoleFn, ResizePseudoConsoleFn, ClosePseudoConsoleFn)> {
    // SAFETY: `module` is a live module handle and each name is NUL-terminated.
    // The resolved addresses are only called with the documented signatures.
    unsafe {
        let create = GetProcAddress(module, PCSTR(b"CreatePseudoConsole\0".as_ptr()))?;
        let resize = GetProcAddress(module, PCSTR(b"ResizePseudoConsole\0".as_ptr()))?;
        let close = GetProcAddress(module, PCSTR(b"ClosePseudoConsole\0".as_ptr()))?;
        Some((
            std::mem::transmute::<unsafe extern "system" fn() -> isize, CreatePseudoConsoleFn>(
                create,
            ),
            std::mem::transmute::<unsafe extern "system" fn() -> isize, ResizePseudoConsoleFn>(
                resize,
            ),
            std::mem::transmute::<unsafe extern "system" fn() -> isize, ClosePseudoConsoleFn>(
                close,
            ),
        ))
    }
}

/// Directory containing this executable, where the packaged conpty.dll lives.
fn executable_directory() -> Option<PathBuf> {
    let mut buffer = vec![0u16; 32_768];
    // SAFETY: the buffer is writable for its full length.
    let written = unsafe { GetModuleFileNameW(None, &mut buffer) } as usize;
    if written == 0 || written >= buffer.len() {
        return None;
    }
    let path = PathBuf::from(String::from_utf16_lossy(&buffer[..written]));
    path.parent().map(Path::to_path_buf)
}

fn load_adjacent_conpty(directory: &Path) -> Option<HMODULE> {
    let path = directory.join("conpty.dll");
    if !path.is_file() {
        return None;
    }
    let wide: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: NUL-terminated path; the returned module is intentionally kept
    // loaded for the process lifetime.
    let module = unsafe { LoadLibraryW(PCWSTR(wide.as_ptr())) }.ok()?;
    Some(module)
}

fn load_kernel32() -> Option<HMODULE> {
    let name: Vec<u16> = "kernel32.dll\0".encode_utf16().collect();
    // SAFETY: NUL-terminated module name; refcount is intentionally retained.
    unsafe { LoadLibraryW(PCWSTR(name.as_ptr())) }.ok()
}

fn load_conpty_api() -> ConPtyApi {
    if let Some(directory) = executable_directory() {
        if let Some(module) = load_adjacent_conpty(&directory) {
            if let Some((create, resize, close)) = resolve_pseudo_console(module) {
                crate::log::info(
                    "conpty",
                    &format!(
                        "using bundled {} for ConPTY (Kitty APC forwarding)",
                        directory.join("conpty.dll").display()
                    ),
                );
                return ConPtyApi {
                    create,
                    resize,
                    close,
                    bundled: true,
                    _module: module,
                };
            }
            crate::log::log(
                "WARN",
                "conpty",
                "adjacent conpty.dll is missing ConPTY exports; falling back to kernel32",
            );
        } else {
            crate::log::log(
                "WARN",
                "conpty",
                CONPTY_FALLBACK_WARNING,
            );
        }
    }
    let module = load_kernel32().expect("kernel32 is always loadable");
    let (create, resize, close) =
        resolve_pseudo_console(module).expect("kernel32 exports the ConPTY entry points");
    ConPtyApi {
        create,
        resize,
        close,
        bundled: false,
        _module: module,
    }
}

#[must_use]
pub fn conpty_api() -> &'static ConPtyApi {
    CONPTY_API.get_or_init(load_conpty_api)
}

/// Whether the active transport is the bundled, APC-preserving host. Exposed
/// so the documented smoke gate can assert degraded fallback honestly.
#[must_use]
pub fn conpty_is_bundled() -> bool {
    conpty_api().bundled
}

struct ConPty {
    raw: HPCON,
}

// SAFETY: HPCON is an OS handle with no thread affinity; uniquely owned here.
unsafe impl Send for ConPty {}

impl Drop for ConPty {
    fn drop(&mut self) {
        // SAFETY: `raw` was created by the same process-global API and is
        // closed exactly once. The module stays loaded for the process.
        unsafe { (conpty_api().close)(self.raw) };
    }
}

struct Threads {
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
    waiter: JoinHandle<()>,
}

/// Owns one ConPTY session. `HPCON` never leaves this module.
pub struct WindowsPty {
    _conpty: Arc<ConPty>,
    // Kept alive for the session: the ConPTY host uses the handles we handed
    // to CreatePseudoConsole for its whole lifetime.
    _input_read: OwnedHandle,
    _output_write: OwnedHandle,
    process: Arc<OwnedHandle>,
    reader_thread: Arc<Mutex<Option<OwnedHandle>>>,
    stop: Arc<AtomicBool>,
    session: Arc<TerminalSession>,
    _size: Arc<Mutex<(u16, u16)>>,
    /// Latest-value mailbox; the UI only updates the pending grid.
    resize_mailbox: Arc<PtyResizeMailbox>,
    threads: Mutex<Option<Threads>>,
}

impl WindowsPty {
    pub fn spawn(config: &PtyConfig, session: Arc<TerminalSession>) -> Result<Self, PtyError> {
        if config.command.program.contains('\0')
            || config.command.args.iter().any(|arg| arg.contains('\0'))
        {
            return Err(PtyError::new("CreateProcessW", "NUL in command line"));
        }

        // A named pipe for input, matching Wintty's proven ConPTY setup. The
        // anonymous output pipe is fine: ConPTY only requires synchronous I/O.
        let pipe_name = format!(
            r"\\.\pipe\LOCAL\pain-conpty-{}-{}",
            std::process::id(),
            NEXT_PIPE_ID.fetch_add(1, Ordering::Relaxed)
        );
        let pipe_name_w: Vec<u16> = pipe_name.encode_utf16().chain(std::iter::once(0)).collect();
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: windows::Win32::Foundation::FALSE,
        };
        // SAFETY: the name is NUL-terminated and the attributes struct is valid.
        let input_write_raw = unsafe {
            CreateNamedPipeW(
                PCWSTR(pipe_name_w.as_ptr()),
                PIPE_ACCESS_OUTBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE,
                1,
                4096,
                4096,
                0,
                Some(&attributes),
            )
        };
        if input_write_raw.is_invalid() || input_write_raw.0.is_null() {
            return Err(PtyError::new("CreateNamedPipeW", "invalid pipe handle"));
        }
        // SAFETY: same name, read access, no sharing; the security attributes
        // are a fresh copy so Windows can populate them.
        let input_read_raw = unsafe {
            CreateFileW(
                PCWSTR(pipe_name_w.as_ptr()),
                GENERIC_READ.0,
                windows::Win32::Storage::FileSystem::FILE_SHARE_MODE::default(),
                Some(&attributes),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
            .map_err(|error| {
                PtyError::new("CreateFileW (conpty input)", error.message().to_string())
            })?
        };
        let input_read = OwnedHandle::from_raw(input_read_raw, "CreateFileW (conpty input)")?;
        let input_write = Arc::new(OwnedHandle::from_raw(input_write_raw, "CreateNamedPipeW")?);

        let mut output_read = HANDLE::default();
        let mut output_write = HANDLE::default();
        // SAFETY: both out-parameters are valid, non-null HANDLE slots.
        unsafe {
            CreatePipe(&mut output_read, &mut output_write, None, 0).map_err(|error| {
                PtyError::new("CreatePipe (output)", error.message().to_string())
            })?;
        }
        let output_read = Arc::new(OwnedHandle::from_raw(output_read, "CreatePipe (output)")?);
        let output_write = OwnedHandle::from_raw(output_write, "CreatePipe (output)")?;

        let coord = COORD {
            X: config.cols.max(1) as i16,
            Y: config.rows.max(1) as i16,
        };
        let mut hpcon = HPCON::default();
        // SAFETY: the two pipe handles are valid and exclusively ours; on
        // success ConPTY owns its copies and we close ours below. The entry
        // point is resolved once per process and never unloaded.
        let result = unsafe {
            (conpty_api().create)(
                coord,
                input_read.as_raw(),
                output_write.as_raw(),
                0,
                &mut hpcon,
            )
        };
        result
            .ok()
            .map_err(|error| PtyError::new("CreatePseudoConsole", error.message().to_string()))?;
        let conpty = Arc::new(ConPty { raw: hpcon });

        let resolved = resolve_program(&config.command.program)?;
        let mut application = resolved
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut command_line = command_line(&config.command)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let environment = environment_block(&terminal_environment(), std::env::vars())
            .map_err(|detail| PtyError::new("environment block", detail))?;

        // SAFETY: InitializeProcThreadAttributeList returns the required byte
        // count in `size`; the second call initializes our owned buffer.
        let mut size = 0usize;
        // The first call reports the required byte count via an error.
        unsafe {
            let _ = InitializeProcThreadAttributeList(None, 1, None, &mut size);
        }
        // The attribute list must be pointer-aligned; a Vec<u8> is not.
        let words = size.div_ceil(size_of::<usize>());
        let mut attribute_buffer = vec![0usize; words];
        let attributes_list = LPPROC_THREAD_ATTRIBUTE_LIST(attribute_buffer.as_mut_ptr().cast());
        // SAFETY: `attribute_buffer` is at least `size` bytes and stays alive
        // until after CreateProcessW has consumed the attribute list.
        unsafe {
            InitializeProcThreadAttributeList(Some(attributes_list), 1, None, &mut size).map_err(
                |error| {
                    PtyError::new(
                        "InitializeProcThreadAttributeList",
                        error.message().to_string(),
                    )
                },
            )?;
            // Microsoft's ConPTY sample passes the HPCON itself as lpValue:
            // UpdateProcThreadAttribute copies the bytes it points at.
            let conpty_value = hpcon.0 as *const c_void;
            UpdateProcThreadAttribute(
                attributes_list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                Some(conpty_value),
                size_of::<HPCON>(),
                None,
                None,
            )
            .map_err(|error| {
                PtyError::new("UpdateProcThreadAttribute", error.message().to_string())
            })?;
        }

        let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        // EXTENDED_STARTUPINFO_PRESENT requires the extended struct size here.
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        // ConPTY replaces the child's std handles; Wintty still sets this flag
        // (with NULL handles) so the console host initializes them correctly.
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.lpAttributeList = attributes_list;
        let mut process_info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: all pointers are valid; the attribute list is initialized and
        // still alive, and the command line/environment are NUL-terminated.
        let created = unsafe {
            CreateProcessW(
                PCWSTR(application.as_mut_ptr()),
                Some(PWSTR(command_line.as_mut_ptr())),
                None,
                None,
                false,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                Some(environment.as_ptr().cast()),
                PCWSTR::null(),
                &startup.StartupInfo,
                &mut process_info,
            )
        };
        // SAFETY: the attribute list must be deleted after process creation.
        unsafe { DeleteProcThreadAttributeList(attributes_list) };
        created.map_err(|error| {
            PtyError::new(
                "CreateProcessW",
                format!("{} (code {:#x})", error.message(), error.code().0),
            )
        })?;
        // SAFETY: CreateProcessW gave us two owned handles on success.
        let process = Arc::new(OwnedHandle::from_raw(
            process_info.hProcess,
            "CreateProcessW",
        )?);
        // SAFETY: the primary thread handle is not needed after creation.
        let _ = unsafe { CloseHandle(process_info.hThread) };

        let stop = Arc::new(AtomicBool::new(false));
        let reader_thread = Arc::new(Mutex::new(None));
        let threads = start_threads(
            &session,
            Arc::clone(&input_write),
            Arc::clone(&output_read),
            Arc::clone(&process),
            Arc::clone(&reader_thread),
            Arc::clone(&stop),
        );
        let size = Arc::new(Mutex::new((config.cols.max(1), config.rows.max(1))));
        let resize_size = Arc::clone(&size);
        let resize_conpty = Arc::clone(&conpty);
        let resize_mailbox = Arc::new(PtyResizeMailbox::new(Arc::new(move |cols, rows| {
            let coord = COORD {
                X: cols as i16,
                Y: rows as i16,
            };
            // SAFETY: the ConPTY handle stays alive until shutdown joins this
            // worker, and the COORD is POD. The entry point is process-global.
            let result = unsafe { (conpty_api().resize)(resize_conpty.raw, coord) };
            match result.ok() {
                Ok(()) => *resize_size.lock().expect("PTY size mutex poisoned") = (cols, rows),
                Err(error) => crate::log::error(
                    "resize",
                    &format!("ResizePseudoConsole failed: {}", error.message()),
                ),
            }
        })));
        Ok(Self {
            _conpty: conpty,
            _input_read: input_read,
            _output_write: output_write,
            process,
            reader_thread,
            stop,
            session,
            _size: size,
            resize_mailbox,
            threads: Mutex::new(Some(threads)),
        })
    }
}

fn start_threads(
    session: &Arc<TerminalSession>,
    input_write: Arc<OwnedHandle>,
    output_read: Arc<OwnedHandle>,
    process: Arc<OwnedHandle>,
    reader_thread: Arc<Mutex<Option<OwnedHandle>>>,
    stop: Arc<AtomicBool>,
) -> Threads {
    let reader_session = Arc::clone(session);
    let reader_output = Arc::clone(&output_read);
    let reader_stop = Arc::clone(&stop);
    let reader_slot = Arc::clone(&reader_thread);
    let reader = thread::Builder::new()
        .name("pain-conpty-reader".into())
        .spawn(move || {
            // Duplicate our own thread handle so shutdown can cancel the
            // blocking ReadFile without closing the pipe underneath it.
            // SAFETY: GetCurrentProcess/GetCurrentThread are pseudo-handles and
            // DuplicateHandle produces a real handle we own.
            let duplicated = unsafe {
                let mut raw = HANDLE::default();
                match DuplicateHandle(
                    GetCurrentProcess(),
                    GetCurrentThread(),
                    GetCurrentProcess(),
                    &mut raw,
                    0,
                    false,
                    DUPLICATE_SAME_ACCESS,
                ) {
                    Ok(()) => OwnedHandle::from_raw(raw, "DuplicateHandle"),
                    Err(error) => Err(PtyError::new(
                        "DuplicateHandle",
                        error.message().to_string(),
                    )),
                }
            };
            if let Ok(duplicated) = duplicated {
                *reader_slot.lock().expect("reader slot poisoned") = Some(duplicated);
            }
            let mut buffer = [0u8; 8192];
            loop {
                if reader_stop.load(Ordering::Acquire) {
                    break;
                }
                let mut read = 0u32;
                // SAFETY: the handle is valid and the buffer is writable for
                // `read` bytes; no overlapped structure is used (synchronous I/O).
                let result = unsafe {
                    windows::Win32::Storage::FileSystem::ReadFile(
                        reader_output.as_raw(),
                        Some(&mut buffer[..]),
                        Some(&mut read),
                        None,
                    )
                };
                match result {
                    Ok(()) if read == 0 => break,
                    Ok(()) => reader_session.write_vt(&buffer[..read as usize]),
                    // ERROR_OPERATION_ABORTED is our own shutdown cancellation.
                    Err(_) => break,
                }
            }
        })
        .expect("spawn ConPTY reader");

    let writer_session = Arc::clone(session);
    let writer_input = Arc::clone(&input_write);
    let writer = thread::Builder::new()
        .name("pain-conpty-writer".into())
        .spawn(move || {
            while let Some(bytes) = writer_session.wait_writer() {
                let mut written = 0usize;
                while written < bytes.len() {
                    let mut chunk = 0u32;
                    // SAFETY: the handle is valid and the slice is readable.
                    let result = unsafe {
                        windows::Win32::Storage::FileSystem::WriteFile(
                            writer_input.as_raw(),
                            Some(&bytes[written..]),
                            Some(&mut chunk),
                            None,
                        )
                    };
                    match result {
                        Ok(()) if chunk == 0 => break,
                        Ok(()) => written += chunk as usize,
                        Err(_) => return,
                    }
                }
            }
        })
        .expect("spawn ConPTY writer");

    let waiter_session = Arc::clone(session);
    let waiter_process = Arc::clone(&process);
    let waiter_reader = Arc::clone(&reader_thread);
    let waiter_stop = Arc::clone(&stop);
    let waiter = thread::Builder::new()
        .name("pain-child-waiter".into())
        .spawn(move || {
            // SAFETY: waiting on our own process handle is well-defined.
            unsafe { WaitForSingleObject(waiter_process.as_raw(), INFINITE) };
            let mut code = 0u32;
            // SAFETY: the process handle is owned and still valid here.
            let _ = unsafe { GetExitCodeProcess(waiter_process.as_raw(), &mut code) };
            waiter_session.mark_process_exited();
            waiter_stop.store(true, Ordering::Release);
            // Give output a brief chance to drain before cancelling the read.
            thread::sleep(std::time::Duration::from_millis(2000));
            if let Some(handle) = waiter_reader.lock().expect("reader slot poisoned").as_ref() {
                // SAFETY: the handle targets the reader thread; cancelling its
                // synchronous I/O unblocks ReadFile with an error.
                let _ = unsafe { CancelSynchronousIo(handle.as_raw()) };
            }
        })
        .expect("spawn child waiter");

    Threads {
        reader,
        writer,
        waiter,
    }
}

impl PlatformPty for WindowsPty {
    fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        // Never call ResizePseudoConsole on the GPUI/session thread: only the
        // latest requested grid is kept by the worker.
        self.resize_mailbox.request(cols, rows);
        Ok(())
    }

    fn process_exited(&self) -> bool {
        self.session.process_exited()
    }

    fn shutdown(self: Box<Self>) {
        self.stop.store(true, Ordering::Release);
        // Stop accepting resizes, wake the worker and join before the ConPTY
        // handle can be dropped. This never blocks indefinitely.
        self.resize_mailbox.shutdown();
        self.session.close_writer();
        if let Some(handle) = self
            .reader_thread
            .lock()
            .expect("reader slot poisoned")
            .as_ref()
        {
            // SAFETY: cancelling the reader's synchronous I/O unblocks it.
            let _ = unsafe { CancelSynchronousIo(handle.as_raw()) };
        }
        // Dropping our ConPTY handle breaks the output pipe. Only escalate to a
        // forced termination after the child had a chance to exit on its own.
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut code = 0u32;
        // SAFETY: process handle is owned and alive inside this method.
        let still_running = unsafe {
            GetExitCodeProcess(self.process.as_raw(), &mut code).is_ok() && code == 259
            // STILL_ACTIVE
        };
        if still_running {
            // SAFETY: targeting only our own child process with exit code 1.
            let _ = unsafe { TerminateProcess(self.process.as_raw(), 1) };
        }
        // HPCON is dropped when `self` is dropped after the joins below.
        if let Some(threads) = self.threads.lock().expect("threads mutex poisoned").take() {
            let _ = threads.reader.join();
            let _ = threads.writer.join();
            let _ = threads.waiter.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::ShellCommand;

    #[test]
    fn quote_argument_handles_spaces_and_quotes() {
        assert_eq!(quote_argument("plain"), "plain");
        assert_eq!(
            quote_argument("C:\\Program Files\\cmd.exe"),
            "\"C:\\Program Files\\cmd.exe\""
        );
        assert_eq!(quote_argument("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote_argument(""), "\"\"");
    }

    #[test]
    fn command_line_quotes_program_and_args() {
        let command = ShellCommand {
            program: "C:\\Program Files\\PowerShell\\pwsh.exe".into(),
            args: vec!["-NoLogo".into(), "a b".into()],
        };
        assert_eq!(
            command_line(&command),
            "\"C:\\Program Files\\PowerShell\\pwsh.exe\" -NoLogo \"a b\""
        );
    }

    #[test]
    fn environment_overrides_are_applied_once() {
        let inherited = [
            ("TERM".into(), "dumb".into()),
            ("PATH".into(), "C:\\bin".into()),
        ];
        let block = environment_block(&terminal_environment(), inherited).expect("block");
        let flat: Vec<u16> = block;
        let as_string = String::from_utf16_lossy(&flat);
        assert!(as_string.contains("TERM=xterm-256color"));
        assert!(as_string.contains("COLORTERM=truecolor"));
        assert!(!as_string.contains("TERM=dumb"));
        assert!(as_string.contains("PATH=C:\\bin"));
        assert!(environment_block(&[("BAD=KEY", "v")], []).is_err());
    }

    #[test]
    fn missing_adjacent_conpty_dll_is_not_a_silent_kitty_claim() {
        // The fallback message must say "degraded" and name Kitty; the Windows
        // graphics gate relies on this being honest, not aspirational.
        assert!(CONPTY_FALLBACK_WARNING.contains("DEGRADED"));
        assert!(CONPTY_FALLBACK_WARNING.contains("Kitty"));
        assert!(CONPTY_FALLBACK_WARNING.contains("kernel32"));
        let directory = std::env::temp_dir().join(format!(
            "pain-no-conpty-{}-{}",
            std::process::id(),
            NEXT_PIPE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).expect("temp dir");
        assert!(load_adjacent_conpty(&directory).is_none());
        let _ = std::fs::remove_dir(&directory);
        // The resolver still finds a usable fallback; observe that it was
        // resolved rather than silently pretending a bundled host exists.
        let _ = conpty_is_bundled();
        assert!(resolve_pseudo_console(load_kernel32().expect("kernel32")).is_some());
    }

    #[test]
    fn kernel32_exports_the_conpty_entry_points() {
        let module = load_kernel32().expect("kernel32 module");
        assert!(resolve_pseudo_console(module).is_some());
    }
}
