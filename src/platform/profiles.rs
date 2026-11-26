//! Shell profile discovery and the Windows jump-list entry model.
//!
//! A profile is a shell command with a stable id. The id is persisted in the
//! preferences document and appears in `--profile`, so it must not change when
//! a label or installation path does.
//!
//! Windows detection deliberately uses well-known install locations first and
//! PATH second. The devkitPro MSYS2 bash and Git Bash are both `bash.exe`, so
//! treating every `bash.exe` on PATH as Git Bash would mislabel the first one
//! found on this machine.

#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(target_os = "windows")]
use std::path::PathBuf;

use crate::platform::ShellCommand;

/// PowerShell 7 (`pwsh.exe`).
pub const PWSH7_ID: &str = "pwsh7";
/// Windows PowerShell 5.1 (`powershell.exe`).
pub const PWSH5_ID: &str = "pwsh5";
/// The in-box command interpreter.
pub const CMD_ID: &str = "cmd";
/// Git for Windows' login bash.
pub const GIT_BASH_ID: &str = "git-bash";
/// devkitPro's MSYS2 bash.
pub const DEVKITPRO_BASH_ID: &str = "devkitpro-bash";
/// WSL without a named distribution (only used when none is registered).
pub const WSL_ID: &str = "wsl";
/// Prefix for per-distribution WSL profile ids.
pub const WSL_ID_PREFIX: &str = "wsl-";

/// One shell that pain can launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellProfile {
    /// Stable id used by preferences and `--profile`.
    pub id: String,
    /// Human-readable name used by Preferences and the taskbar menu.
    pub label: String,
    pub command: ShellCommand,
}

// Only the Windows detector needs this constructor; Linux builds a single
// login profile directly and must not trip `-D dead-code` in CI.
#[cfg(target_os = "windows")]
impl ShellProfile {
    fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        program: PathBuf,
        args: &[&str],
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            command: ShellCommand {
                program: program.to_string_lossy().into_owned(),
                args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            },
        }
    }
}

/// Find a profile by its persisted id.
#[must_use]
pub fn find_profile<'a>(profiles: &'a [ShellProfile], id: &str) -> Option<&'a ShellProfile> {
    profiles.iter().find(|profile| profile.id == id)
}

/// The platform's automatic choice. Windows prefers PowerShell 7, then
/// Windows PowerShell, then the command interpreter; Linux uses the login
/// shell that was detected first.
#[must_use]
pub fn default_profile_id(profiles: &[ShellProfile]) -> Option<&str> {
    #[cfg(target_os = "windows")]
    for preferred in [PWSH7_ID, PWSH5_ID, CMD_ID] {
        if let Some(profile) = find_profile(profiles, preferred) {
            return Some(profile.id.as_str());
        }
    }
    profiles.first().map(|profile| profile.id.as_str())
}

/// One taskbar jump-list entry. Kept pure so the profile-to-argument mapping
/// is testable without COM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JumpListTask {
    pub title: String,
    pub arguments: String,
}

/// Turn every detected profile into a taskbar task. The argument is quoted
/// even though ids are slugged, because a future profile source may not be.
#[must_use]
pub fn jump_list_tasks(profiles: &[ShellProfile]) -> Vec<JumpListTask> {
    profiles
        .iter()
        .map(|profile| JumpListTask {
            title: format!("New {} window", profile.label),
            arguments: format!("--profile \"{}\"", profile.id),
        })
        .collect()
}

#[cfg(target_os = "linux")]
#[must_use]
pub fn detect_profiles() -> Vec<ShellProfile> {
    let command = crate::platform::default_shell();
    let name = Path::new(&command.program)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| command.program.clone());
    vec![ShellProfile {
        id: "login".to_owned(),
        label: format!("Login shell ({name})"),
        command,
    }]
}

#[cfg(target_os = "windows")]
#[must_use]
pub fn detect_profiles() -> Vec<ShellProfile> {
    detect_windows_profiles(&WindowsLayout::from_environment(), &wsl_distros())
}

#[cfg(target_os = "windows")]
pub fn install_jump_list(profiles: &[ShellProfile]) {
    if profiles.is_empty() {
        return;
    }
    match try_install_jump_list(profiles) {
        Ok(()) => crate::log::info(
            "jump-list",
            &format!("installed {} shell profile tasks", profiles.len()),
        ),
        Err(error) => crate::log::error("jump-list", &format!("{error}")),
    }
}

#[cfg(target_os = "windows")]
#[derive(Debug, Default)]
struct WindowsLayout {
    path_dirs: Vec<PathBuf>,
    program_files: Option<PathBuf>,
    program_files_x86: Option<PathBuf>,
    local_app_data: Option<PathBuf>,
    user_profile: Option<PathBuf>,
    system_root: Option<PathBuf>,
    comspec: Option<PathBuf>,
    devkitpro: Option<PathBuf>,
}

#[cfg(target_os = "windows")]
impl WindowsLayout {
    fn from_environment() -> Self {
        let path_dirs = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        Self {
            path_dirs,
            program_files: env_path("ProgramFiles"),
            program_files_x86: env_path("ProgramFiles(x86)"),
            local_app_data: env_path("LOCALAPPDATA"),
            user_profile: env_path("USERPROFILE"),
            system_root: env_path("SystemRoot"),
            comspec: env_path("ComSpec"),
            devkitpro: env_path("DEVKITPRO"),
        }
    }
}

#[cfg(target_os = "windows")]
fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

#[cfg(target_os = "windows")]
fn first_file(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find(|path| path.is_file())
}

#[cfg(target_os = "windows")]
fn in_path(layout: &WindowsLayout, file_name: &str) -> Vec<PathBuf> {
    layout
        .path_dirs
        .iter()
        .map(|dir| dir.join(file_name))
        .collect()
}

#[cfg(target_os = "windows")]
fn pwsh7_candidates(layout: &WindowsLayout) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(program_files) = &layout.program_files {
        candidates.push(program_files.join("PowerShell/7/pwsh.exe"));
        candidates.push(program_files.join("PowerShell/7-preview/pwsh.exe"));
    }
    candidates.extend(in_path(layout, "pwsh.exe"));
    if let Some(local_app_data) = &layout.local_app_data {
        candidates.push(local_app_data.join("Microsoft/WindowsApps/pwsh.exe"));
    }
    candidates
}

#[cfg(target_os = "windows")]
fn pwsh5_candidates(layout: &WindowsLayout) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(system_root) = &layout.system_root {
        candidates.push(system_root.join("System32/WindowsPowerShell/v1.0/powershell.exe"));
    }
    candidates.extend(in_path(layout, "powershell.exe"));
    candidates
}

#[cfg(target_os = "windows")]
fn cmd_candidates(layout: &WindowsLayout) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(comspec) = &layout.comspec {
        candidates.push(comspec.clone());
    }
    if let Some(system_root) = &layout.system_root {
        candidates.push(system_root.join("System32/cmd.exe"));
    }
    candidates.extend(in_path(layout, "cmd.exe"));
    candidates
}

#[cfg(target_os = "windows")]
fn git_bash_candidates(layout: &WindowsLayout) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(program_files) = &layout.program_files {
        candidates.push(program_files.join("Git/bin/bash.exe"));
        candidates.push(program_files.join("Git/usr/bin/bash.exe"));
    }
    if let Some(program_files_x86) = &layout.program_files_x86 {
        candidates.push(program_files_x86.join("Git/bin/bash.exe"));
    }
    if let Some(local_app_data) = &layout.local_app_data {
        candidates.push(local_app_data.join("Programs/Git/bin/bash.exe"));
    }
    if let Some(user_profile) = &layout.user_profile {
        candidates.push(user_profile.join("scoop/apps/git/current/bin/bash.exe"));
    }
    candidates
}

#[cfg(target_os = "windows")]
fn devkitpro_bash_candidates(layout: &WindowsLayout) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(devkitpro) = &layout.devkitpro {
        candidates.push(devkitpro.join("msys2/usr/bin/bash.exe"));
    }
    candidates.push(PathBuf::from(r"C:\devkitPro\msys2\usr\bin\bash.exe"));
    candidates
}

#[cfg(target_os = "windows")]
fn wsl_candidates(layout: &WindowsLayout) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(system_root) = &layout.system_root {
        candidates.push(system_root.join("System32/wsl.exe"));
    }
    candidates.extend(in_path(layout, "wsl.exe"));
    candidates
}

#[cfg(target_os = "windows")]
fn detect_windows_profiles(layout: &WindowsLayout, wsl_distros: &[String]) -> Vec<ShellProfile> {
    let mut profiles = Vec::new();
    if let Some(path) = first_file(pwsh7_candidates(layout)) {
        profiles.push(ShellProfile::new(
            PWSH7_ID,
            "PowerShell 7",
            path,
            &["-NoLogo"],
        ));
    }
    if let Some(path) = first_file(pwsh5_candidates(layout)) {
        profiles.push(ShellProfile::new(
            PWSH5_ID,
            "Windows PowerShell",
            path,
            &["-NoLogo"],
        ));
    }
    if let Some(path) = first_file(cmd_candidates(layout)) {
        profiles.push(ShellProfile::new(CMD_ID, "Command Prompt", path, &[]));
    }
    if let Some(path) = first_file(git_bash_candidates(layout)) {
        profiles.push(ShellProfile::new(
            GIT_BASH_ID,
            "Git Bash",
            path,
            &["--login", "-i"],
        ));
    }
    if let Some(path) = first_file(devkitpro_bash_candidates(layout)) {
        profiles.push(ShellProfile::new(
            DEVKITPRO_BASH_ID,
            "devkitPro Bash",
            path,
            &["--login", "-i"],
        ));
    }
    if let Some(path) = first_file(wsl_candidates(layout)) {
        if wsl_distros.is_empty() {
            profiles.push(ShellProfile::new(WSL_ID, "WSL", path, &[]));
        } else {
            let mut used = Vec::new();
            for distro in wsl_distros {
                let id = unique_wsl_id(distro, &used);
                used.push(id.clone());
                profiles.push(ShellProfile::new(
                    id,
                    format!("WSL: {distro}"),
                    path.clone(),
                    &["-d", distro.as_str(), "--cd", "~"],
                ));
            }
        }
    }
    profiles
}

/// Slug a distribution name for use in a command line and JSON. The suffix
/// keeps two names that slug identically from sharing one persisted id.
#[cfg(target_os = "windows")]
fn unique_wsl_id(distro: &str, used: &[String]) -> String {
    let base = slug(WSL_ID_PREFIX, distro);
    if !used.iter().any(|id| id == &base) {
        return base;
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !used.iter().any(|id| id == &candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

#[cfg(target_os = "windows")]
fn slug(prefix: &str, name: &str) -> String {
    let mut id = String::with_capacity(prefix.len() + name.len());
    id.push_str(prefix);
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            id.push(character.to_ascii_lowercase());
        } else if !id.ends_with('-') {
            id.push('-');
        }
    }
    while id.ends_with('-') {
        id.pop();
    }
    id
}

/// Read installed WSL distributions from the per-user registry. This is
/// cheaper and quieter than spawning `wsl.exe -l` from a GUI process.
#[cfg(target_os = "windows")]
fn wsl_distros() -> Vec<String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
        RRF_RT_REG_SZ,
    };

    let mut key = HKEY::default();
    // SAFETY: the subkey is a NUL-terminated UTF-16 literal. On success the
    // returned key is closed before this function returns.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            windows::core::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Lxss"),
            Some(0),
            KEY_READ,
            &mut key,
        )
    };
    if opened != ERROR_SUCCESS {
        return Vec::new();
    }

    let mut distros = Vec::new();
    let mut index = 0u32;
    loop {
        let mut name = [0u16; 256];
        let mut name_len = name.len() as u32;
        // SAFETY: name is a valid writable buffer and name_len is its length
        // in UTF-16 units. All optional output parameters are omitted.
        let result = unsafe {
            RegEnumKeyExW(
                key,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                None,
                None,
                None,
            )
        };
        if result == ERROR_NO_MORE_ITEMS {
            break;
        }
        if result != ERROR_SUCCESS {
            break;
        }

        let mut value = [0u16; 512];
        let mut value_bytes = (value.len() * 2) as u32;
        // SAFETY: `value` is a writable UTF-16 buffer and `value_bytes` is its
        // size in bytes. `name` is NUL-terminated from the buffer above.
        let result = unsafe {
            RegGetValueW(
                key,
                PCWSTR(name.as_ptr()),
                windows::core::w!("DistributionName"),
                RRF_RT_REG_SZ,
                None,
                Some(value.as_mut_ptr().cast()),
                Some(&mut value_bytes),
            )
        };
        if result == ERROR_SUCCESS && value_bytes >= 2 {
            let characters = (value_bytes as usize / 2).saturating_sub(1);
            distros.push(String::from_utf16_lossy(&value[..characters]));
        }
        index += 1;
    }

    // SAFETY: `key` was opened successfully above and is not used afterwards.
    unsafe {
        let _ = RegCloseKey(key);
    }
    distros
}

#[cfg(target_os = "windows")]
fn try_install_jump_list(profiles: &[ShellProfile]) -> windows::core::Result<()> {
    use windows::core::{Interface, HSTRING};
    use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::Common::IObjectCollection;
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::Win32::UI::Shell::{
        DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW, ShellLink,
    };

    let exe_path = HSTRING::from(std::env::current_exe()?.as_os_str());
    let list: ICustomDestinationList =
        unsafe { CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER) }?;
    let mut slots = 0u32;
    let removed: windows::Win32::UI::Shell::Common::IObjectArray =
        unsafe { list.BeginList(&mut slots) }?;
    drop(removed);
    let tasks: IObjectCollection =
        unsafe { CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER) }?;

    for task in jump_list_tasks(profiles) {
        let argument = HSTRING::from(task.arguments);
        let title = HSTRING::from(task.title.as_str());
        let link: IShellLinkW =
            unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }?;
        // SAFETY: every setter copies its HSTRING before returning. The
        // property store is the same native ShellLink object, so the cast is a
        // QueryInterface against a genuinely supported interface.
        unsafe {
            link.SetPath(&exe_path)?;
            link.SetArguments(&argument)?;
            link.SetDescription(&title)?;
            let store: IPropertyStore = link.cast()?;
            let mut value = PROPVARIANT::from(task.title.as_str());
            let set = store.SetValue(&PKEY_TITLE, &value);
            let _ = PropVariantClear(&mut value);
            set?;
            store.Commit()?;
            tasks.AddObject(&link)?;
        }
    }

    unsafe {
        list.AddUserTasks(&tasks)?;
        list.CommitList()?;
    }
    Ok(())
}

/// System.Title. The shell uses this property, not the link description, for
/// the visible jump-list label. Value copied from the Windows SDK's
/// FMTID_SummaryInformation/PIDSI_TITLE entry.
#[cfg(target_os = "windows")]
const PKEY_TITLE: windows::Win32::Foundation::PROPERTYKEY =
    windows::Win32::Foundation::PROPERTYKEY {
        fmtid: windows::core::GUID::from_u128(0xf29f85e0_4ff9_1068_ab91_08002b27b3d9),
        pid: 2,
    };

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str, label: &str) -> ShellProfile {
        ShellProfile {
            id: id.to_owned(),
            label: label.to_owned(),
            command: ShellCommand::new(format!("{id}.exe")),
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_default_prefers_pwsh7_then_pwsh5_then_cmd() {
        let all = vec![
            profile("cmd", "Command Prompt"),
            profile("pwsh5", "Windows PowerShell"),
        ];
        assert_eq!(default_profile_id(&all), Some(PWSH5_ID));

        let with_pwsh7 = vec![
            profile("cmd", "Command Prompt"),
            profile("pwsh5", "Windows PowerShell"),
            profile("pwsh7", "PowerShell 7"),
        ];
        assert_eq!(default_profile_id(&with_pwsh7), Some(PWSH7_ID));

        let only_cmd = vec![profile("cmd", "Command Prompt")];
        assert_eq!(default_profile_id(&only_cmd), Some(CMD_ID));
    }

    #[test]
    fn live_detection_returns_a_usable_default() {
        let profiles = detect_profiles();
        assert!(!profiles.is_empty(), "at least one shell must be found");
        let default_id = default_profile_id(&profiles).expect("a default profile");
        assert!(
            find_profile(&profiles, default_id).is_some(),
            "the default id must resolve to a detected profile"
        );
    }

    #[test]
    fn jump_list_task_arguments_select_the_profile() {
        let tasks = jump_list_tasks(&[
            profile("pwsh7", "PowerShell 7"),
            profile("wsl-fedora", "WSL: Fedora"),
        ]);
        assert_eq!(tasks[0].title, "New PowerShell 7 window");
        assert_eq!(tasks[0].arguments, "--profile \"pwsh7\"");
        assert_eq!(tasks[1].arguments, "--profile \"wsl-fedora\"");
    }

    #[cfg(target_os = "windows")]
    mod windows {
        use super::*;
        use std::fs;
        use std::path::Path;

        fn touch(path: &Path) {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create fake install directory");
            }
            fs::write(path, b"").expect("create fake executable");
        }

        #[test]
        fn detection_uses_well_known_locations_and_keeps_profiles_distinct() {
            let root = std::env::temp_dir().join(format!("pain-profiles-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let program_files = root.join("ProgramFiles");
            let local_app_data = root.join("LocalAppData");
            let devkitpro = root.join("devkitPro");
            let system_root = root.join("Windows");
            let user_profile = root.join("User");

            touch(&program_files.join("Git/bin/bash.exe"));
            touch(&devkitpro.join("msys2/usr/bin/bash.exe"));
            touch(&local_app_data.join("Microsoft/WindowsApps/pwsh.exe"));
            touch(&system_root.join("System32/WindowsPowerShell/v1.0/powershell.exe"));
            touch(&system_root.join("System32/cmd.exe"));
            touch(&system_root.join("System32/wsl.exe"));

            let layout = WindowsLayout {
                path_dirs: Vec::new(),
                program_files: Some(program_files),
                program_files_x86: None,
                local_app_data: Some(local_app_data),
                user_profile: Some(user_profile),
                system_root: Some(system_root),
                comspec: None,
                devkitpro: Some(devkitpro),
            };
            let profiles = detect_windows_profiles(&layout, &["Ubuntu".to_owned()]);
            let ids: Vec<&str> = profiles.iter().map(|profile| profile.id.as_str()).collect();
            assert_eq!(
                ids,
                [
                    PWSH7_ID,
                    PWSH5_ID,
                    CMD_ID,
                    GIT_BASH_ID,
                    DEVKITPRO_BASH_ID,
                    "wsl-ubuntu",
                ]
            );
            assert_eq!(default_profile_id(&profiles), Some(PWSH7_ID));

            let _ = fs::remove_dir_all(&root);
        }

        #[test]
        fn wsl_ids_are_slugged_and_unique() {
            assert_eq!(slug("wsl-", "Fedora Linux 44"), "wsl-fedora-linux-44");
            let used = vec!["wsl-ubuntu".to_owned()];
            assert_eq!(unique_wsl_id("Ubuntu!", &used), "wsl-ubuntu-2");
        }
    }
}
