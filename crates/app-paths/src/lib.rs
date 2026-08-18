//! Side-effect-free per-user path policy for Steam Controller Bridge.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Component, Path, PathBuf};

const APPLICATION_DIRECTORY: &str = "Steam Controller Bridge";
const UNIX_APPLICATION_DIRECTORY: &str = "steam-controller-bridge";

/// Operating-system path policy to resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
    Windows,
}

impl fmt::Display for Platform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MacOs => "macOS",
            Self::Linux => "Linux",
            Self::Windows => "Windows",
        })
    }
}

/// Resolved application-owned directories.
///
/// Resolution never creates these directories. Consumers remain responsible
/// for creating them with permissions appropriate to the current platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub log_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub runtime_dir: PathBuf,
}

/// Environment inputs used by the pure resolver.
pub trait Env {
    fn var_os(&self, key: &str) -> Option<OsString>;
    fn home_dir(&self) -> Option<PathBuf>;
    fn temp_dir(&self) -> PathBuf;
}

/// Failure to derive a safe application path policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    MissingHome {
        platform: Platform,
    },
    MissingVariable {
        platform: Platform,
        name: &'static str,
    },
    MissingRuntimeIdentity,
    UnsupportedPlatform {
        target: &'static str,
    },
}

impl fmt::Display for PathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHome { platform } => {
                write!(formatter, "home directory is unavailable for {platform}")
            }
            Self::MissingVariable { platform, name } => {
                write!(formatter, "{name} is unavailable for {platform}")
            }
            Self::MissingRuntimeIdentity => formatter
                .write_str("Linux runtime fallback needs USER, LOGNAME, or a named home directory"),
            Self::UnsupportedPlatform { target } => {
                write!(formatter, "application paths are unsupported on {target}")
            }
        }
    }
}

impl std::error::Error for PathError {}

/// Resolves one platform's policy without reading or modifying the host.
///
/// This function intentionally accepts a platform independently of the
/// compilation target so every policy can be covered on every CI host.
///
/// # Errors
/// Returns an error when a required home, environment variable, or safe Linux
/// runtime identity is unavailable.
pub fn resolve(platform: Platform, env: &dyn Env) -> Result<AppPaths, PathError> {
    match platform {
        Platform::MacOs => resolve_macos(env),
        Platform::Linux => resolve_linux(env),
        Platform::Windows => resolve_windows(env),
    }
}

/// Resolves paths for the compilation target using the process environment.
///
/// # Errors
/// Returns an error when the target is unsupported or its required environment
/// inputs are unavailable.
pub fn current() -> Result<AppPaths, PathError> {
    let Some(platform) = current_platform() else {
        return Err(PathError::UnsupportedPlatform {
            target: std::env::consts::OS,
        });
    };
    resolve(platform, &SystemEnv)
}

fn resolve_macos(env: &dyn Env) -> Result<AppPaths, PathError> {
    let home = required_home(env, Platform::MacOs)?;
    let config_dir = home
        .join("Library")
        .join("Application Support")
        .join(APPLICATION_DIRECTORY);
    Ok(AppPaths {
        log_dir: home
            .join("Library")
            .join("Logs")
            .join(APPLICATION_DIRECTORY),
        cache_dir: config_dir.join("Updates"),
        config_dir,
        runtime_dir: env.temp_dir(),
    })
}

fn resolve_linux(env: &dyn Env) -> Result<AppPaths, PathError> {
    let home = env.home_dir();
    let config_home = unix_absolute_var(env, "XDG_CONFIG_HOME")
        .map_or_else(|| home_fallback(home.as_deref(), ".config"), Ok)?;
    let state_home = unix_absolute_var(env, "XDG_STATE_HOME")
        .map_or_else(|| home_fallback(home.as_deref(), ".local/state"), Ok)?;
    let cache_home = unix_absolute_var(env, "XDG_CACHE_HOME")
        .map_or_else(|| home_fallback(home.as_deref(), ".cache"), Ok)?;
    let runtime_dir = match unix_absolute_var(env, "XDG_RUNTIME_DIR") {
        Some(directory) => directory.join(UNIX_APPLICATION_DIRECTORY),
        None => linux_runtime_fallback(env, home.as_deref())?,
    };

    Ok(AppPaths {
        config_dir: config_home.join(UNIX_APPLICATION_DIRECTORY),
        log_dir: state_home.join(UNIX_APPLICATION_DIRECTORY).join("logs"),
        cache_dir: cache_home.join(UNIX_APPLICATION_DIRECTORY),
        runtime_dir,
    })
}

fn resolve_windows(env: &dyn Env) -> Result<AppPaths, PathError> {
    let roaming = required_var(env, Platform::Windows, "APPDATA")?;
    let local = required_var(env, Platform::Windows, "LOCALAPPDATA")?;
    Ok(AppPaths {
        config_dir: PathBuf::from(roaming).join(APPLICATION_DIRECTORY),
        log_dir: PathBuf::from(&local)
            .join(APPLICATION_DIRECTORY)
            .join("Logs"),
        cache_dir: PathBuf::from(&local)
            .join(APPLICATION_DIRECTORY)
            .join("Cache"),
        runtime_dir: PathBuf::from(local)
            .join(APPLICATION_DIRECTORY)
            .join("Runtime"),
    })
}

fn required_home(env: &dyn Env, platform: Platform) -> Result<PathBuf, PathError> {
    env.home_dir()
        .filter(|directory| !directory.as_os_str().is_empty())
        .ok_or(PathError::MissingHome { platform })
}

fn required_var(
    env: &dyn Env,
    platform: Platform,
    name: &'static str,
) -> Result<OsString, PathError> {
    env.var_os(name)
        .filter(|value| !value.is_empty())
        .ok_or(PathError::MissingVariable { platform, name })
}

fn unix_absolute_var(env: &dyn Env, name: &str) -> Option<PathBuf> {
    env.var_os(name)
        .filter(|value| value.as_encoded_bytes().first() == Some(&b'/'))
        .map(PathBuf::from)
}

fn home_fallback(home: Option<&Path>, suffix: &str) -> Result<PathBuf, PathError> {
    home.filter(|directory| !directory.as_os_str().is_empty())
        .map(|directory| directory.join(suffix))
        .ok_or(PathError::MissingHome {
            platform: Platform::Linux,
        })
}

fn linux_runtime_fallback(env: &dyn Env, home: Option<&Path>) -> Result<PathBuf, PathError> {
    let identity = env
        .var_os("USER")
        .filter(|value| is_normal_component(value))
        .or_else(|| {
            env.var_os("LOGNAME")
                .filter(|value| is_normal_component(value))
        })
        .or_else(|| home.and_then(Path::file_name).map(OsStr::to_os_string))
        .filter(|value| is_normal_component(value))
        .ok_or(PathError::MissingRuntimeIdentity)?;
    Ok(env
        .temp_dir()
        .join(UNIX_APPLICATION_DIRECTORY)
        .join(identity))
}

fn is_normal_component(value: &OsStr) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

const fn current_platform() -> Option<Platform> {
    #[cfg(target_os = "macos")]
    {
        return Some(Platform::MacOs);
    }
    #[cfg(target_os = "linux")]
    {
        return Some(Platform::Linux);
    }
    #[cfg(target_os = "windows")]
    {
        return Some(Platform::Windows);
    }
    #[allow(unreachable_code)]
    None
}

struct SystemEnv;

impl Env for SystemEnv {
    fn var_os(&self, key: &str) -> Option<OsString> {
        std::env::var_os(key)
    }

    fn home_dir(&self) -> Option<PathBuf> {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
    }

    fn temp_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[derive(Default)]
    struct FakeEnv {
        vars: BTreeMap<&'static str, OsString>,
        home: Option<PathBuf>,
        temp: PathBuf,
    }

    impl FakeEnv {
        fn macos() -> Self {
            Self {
                home: Some(unix_path(&["Users", "tester"])),
                temp: unix_path(&["private", "var", "folders", "test", "T"]),
                ..Self::default()
            }
        }

        fn linux() -> Self {
            Self {
                home: Some(unix_path(&["home", "tester"])),
                temp: unix_path(&["tmp"]),
                ..Self::default()
            }
        }

        fn windows() -> Self {
            let mut environment = Self::default();
            environment.vars.insert(
                "APPDATA",
                OsString::from(r"C:\Users\tester\AppData\Roaming"),
            );
            environment.vars.insert(
                "LOCALAPPDATA",
                OsString::from(r"C:\Users\tester\AppData\Local"),
            );
            environment
        }

        fn with_var(mut self, key: &'static str, value: &str) -> Self {
            self.vars.insert(key, OsString::from(value));
            self
        }
    }

    fn unix_path(components: &[&str]) -> PathBuf {
        components
            .iter()
            .fold(PathBuf::from("/"), |path, component| path.join(component))
    }

    impl Env for FakeEnv {
        fn var_os(&self, key: &str) -> Option<OsString> {
            self.vars.get(key).cloned()
        }

        fn home_dir(&self) -> Option<PathBuf> {
            self.home.clone()
        }

        fn temp_dir(&self) -> PathBuf {
            self.temp.clone()
        }
    }

    #[test]
    fn macos_policy_preserves_every_shipped_location() {
        let paths = resolve(Platform::MacOs, &FakeEnv::macos()).unwrap();
        let application_support = unix_path(&[
            "Users",
            "tester",
            "Library",
            "Application Support",
            "Steam Controller Bridge",
        ]);
        assert_eq!(paths.config_dir, application_support.clone());
        assert_eq!(
            paths.log_dir,
            unix_path(&[
                "Users",
                "tester",
                "Library",
                "Logs",
                "Steam Controller Bridge",
            ])
        );
        assert_eq!(paths.cache_dir, application_support.join("Updates"));
        assert_eq!(
            paths.runtime_dir,
            unix_path(&["private", "var", "folders", "test", "T"])
        );
        assert_eq!(
            paths.config_dir.join("bindings.json"),
            unix_path(&[
                "Users",
                "tester",
                "Library",
                "Application Support",
                "Steam Controller Bridge",
                "bindings.json",
            ])
        );
        assert_eq!(
            paths.config_dir.join("settings.json"),
            unix_path(&[
                "Users",
                "tester",
                "Library",
                "Application Support",
                "Steam Controller Bridge",
                "settings.json",
            ])
        );
        assert_eq!(
            paths.log_dir.join("sc-bridge.log"),
            unix_path(&[
                "Users",
                "tester",
                "Library",
                "Logs",
                "Steam Controller Bridge",
                "sc-bridge.log",
            ])
        );
    }

    #[test]
    fn macos_requires_a_home_without_touching_the_host() {
        let error = resolve(
            Platform::MacOs,
            &FakeEnv {
                temp: unix_path(&["tmp"]),
                ..FakeEnv::default()
            },
        )
        .unwrap_err();
        assert_eq!(
            error,
            PathError::MissingHome {
                platform: Platform::MacOs
            }
        );
    }

    #[test]
    fn linux_policy_uses_absolute_xdg_directories() {
        let environment = FakeEnv::linux()
            .with_var("XDG_CONFIG_HOME", "/xdg/config")
            .with_var("XDG_STATE_HOME", "/xdg/state")
            .with_var("XDG_CACHE_HOME", "/xdg/cache")
            .with_var("XDG_RUNTIME_DIR", "/run/user/1000");
        assert_eq!(
            resolve(Platform::Linux, &environment).unwrap(),
            AppPaths {
                config_dir: unix_path(&["xdg", "config", "steam-controller-bridge"]),
                log_dir: unix_path(&["xdg", "state", "steam-controller-bridge", "logs"]),
                cache_dir: unix_path(&["xdg", "cache", "steam-controller-bridge"]),
                runtime_dir: unix_path(&["run", "user", "1000", "steam-controller-bridge",]),
            }
        );
    }

    #[test]
    fn linux_policy_uses_standard_defaults_and_a_per_user_runtime_fallback() {
        let environment = FakeEnv::linux().with_var("USER", "tester");
        assert_eq!(
            resolve(Platform::Linux, &environment).unwrap(),
            AppPaths {
                config_dir: unix_path(&["home", "tester", ".config", "steam-controller-bridge",]),
                log_dir: unix_path(&[
                    "home",
                    "tester",
                    ".local",
                    "state",
                    "steam-controller-bridge",
                    "logs",
                ]),
                cache_dir: unix_path(&["home", "tester", ".cache", "steam-controller-bridge",]),
                runtime_dir: unix_path(&["tmp", "steam-controller-bridge", "tester"]),
            }
        );
    }

    #[test]
    fn linux_policy_ignores_relative_xdg_values() {
        let environment = FakeEnv::linux()
            .with_var("USER", "tester")
            .with_var("XDG_CONFIG_HOME", "relative-config")
            .with_var("XDG_STATE_HOME", "relative-state")
            .with_var("XDG_CACHE_HOME", "relative-cache")
            .with_var("XDG_RUNTIME_DIR", "relative-runtime");
        let paths = resolve(Platform::Linux, &environment).unwrap();
        assert_eq!(
            paths.config_dir,
            unix_path(&["home", "tester", ".config", "steam-controller-bridge",])
        );
        assert_eq!(
            paths.runtime_dir,
            unix_path(&["tmp", "steam-controller-bridge", "tester"])
        );
    }

    #[test]
    fn linux_runtime_fallback_rejects_ambiguous_user_identity() {
        let error = resolve(
            Platform::Linux,
            &FakeEnv {
                home: Some(PathBuf::from("/")),
                temp: unix_path(&["tmp"]),
                ..FakeEnv::default()
            },
        )
        .unwrap_err();
        assert_eq!(error, PathError::MissingRuntimeIdentity);
    }

    #[test]
    fn windows_policy_uses_roaming_and_local_application_data() {
        let environment = FakeEnv::windows();
        let paths = resolve(Platform::Windows, &environment).unwrap();
        let roaming = PathBuf::from(r"C:\Users\tester\AppData\Roaming");
        let local = PathBuf::from(r"C:\Users\tester\AppData\Local");
        assert_eq!(paths.config_dir, roaming.join("Steam Controller Bridge"));
        assert_eq!(
            paths.log_dir,
            local.join("Steam Controller Bridge").join("Logs")
        );
        assert_eq!(
            paths.cache_dir,
            local.join("Steam Controller Bridge").join("Cache")
        );
        assert_eq!(
            paths.runtime_dir,
            local.join("Steam Controller Bridge").join("Runtime")
        );
    }

    #[test]
    fn windows_policy_reports_the_missing_environment_variable() {
        let error = resolve(Platform::Windows, &FakeEnv::default()).unwrap_err();
        assert_eq!(
            error,
            PathError::MissingVariable {
                platform: Platform::Windows,
                name: "APPDATA",
            }
        );
    }
}
