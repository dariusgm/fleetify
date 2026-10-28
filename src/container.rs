//! Agent containers: a coding agent runs in a container built from the recipe's `image`
//! (stage 3, first step; see `docs/design/security.md` and `DECISIONS.md`).
//!
//! The workspace, and every granted directory, is mounted at the path it has on the host.
//! The argument list the adapter builds for the host therefore works unchanged, and the
//! prompt is the last argument, passed as one value. The container gets the agent's own
//! variables and nothing else: the orchestrator's environment is not inherited. The
//! variables are named with `-e NAME` and read by the docker client, so their values never
//! appear on a command line.
//!
//! The image must carry the label [`LABEL`] naming the agent it runs, so a recipe cannot run
//! one agent in another agent's image by mistake.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::runner::{BASE_ENV, EnvPolicy};

/// Image label that names the agent the image runs.
pub const LABEL: &str = "io.fleetify.agent";
/// Replaces the `docker` binary, the same way `FLEETIFY_CLAUDE_BIN` does for claude.
pub const DOCKER_BINARY_ENV: &str = "FLEETIFY_DOCKER_BIN";
/// Variables the docker client needs to reach its daemon. They never reach the container.
const CLIENT_ENV: &[&str] = &[
    "DOCKER_HOST",
    "DOCKER_CONFIG",
    "DOCKER_CONTEXT",
    "XDG_RUNTIME_DIR",
];
static NEXT: AtomicU64 = AtomicU64::new(0);

/// The docker client program: `FLEETIFY_DOCKER_BIN` or `docker` on the PATH.
pub fn docker_binary() -> String {
    std::env::var(DOCKER_BINARY_ENV).unwrap_or_else(|_| "docker".to_string())
}

/// Refuses an image reference with anything but letters, digits and `._/:@-`. Such a name
/// cannot be read as an option or split the docker command line.
pub fn check_name(image: &str) -> Result<(), String> {
    let valid = !image.is_empty()
        && !image.starts_with('-')
        && image
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._/:@-".contains(c));
    if valid {
        Ok(())
    } else {
        Err(format!("image {image:?} is not an image name"))
    }
}

/// The environment of a docker client process. `forwarded` names are handed on to the
/// container and are read from here; `set` adds fixed values.
pub fn client_env(forwarded: Vec<String>, set: Vec<(String, String)>) -> EnvPolicy {
    EnvPolicy::Allowlist {
        names: BASE_ENV
            .iter()
            .chain(CLIENT_ENV)
            .map(|name| name.to_string())
            .chain(forwarded)
            .collect(),
        set,
    }
}

/// A fresh container name, unique across runs, so the run can stop its own container.
pub fn container_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!(
        "fleetify-{}-{nanos}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// One agent run in a container.
pub struct Run<'a> {
    pub image: &'a str,
    pub name: &'a str,
    /// The program inside the image, e.g. `claude`.
    pub program: &'a str,
    /// The arguments after the program. The prompt is among them.
    pub args: &'a [String],
    pub workspace: &'a Path,
    /// Granted directories. Each is mounted at its own path, as the workspace is.
    pub directories: &'a [String],
    /// Variable names copied from the docker client's environment.
    pub forwarded: &'a [String],
    /// Host files mounted read-only at the given path in the container: the agent's login.
    /// A file that does not exist on the host is left out by the caller.
    pub files: &'a [(PathBuf, String)],
    /// Paths in the container that get a fresh, empty, writable tmpfs.
    pub tmpfs: &'a [String],
}

/// The `docker run` command line for `run`. The container is removed when it exits, and it
/// gets no capabilities beyond the defaults it needs to run an agent.
pub fn run_argv(run: &Run) -> Result<Vec<String>, String> {
    check_name(run.image)?;
    let workspace = host_path(run.workspace)?;
    let mut argv = vec![
        docker_binary(),
        "run".to_string(),
        "--rm".to_string(),
        "--init".to_string(),
        "--name".to_string(),
        run.name.to_string(),
        "--cap-drop".to_string(),
        "ALL".to_string(),
        "--security-opt".to_string(),
        "no-new-privileges".to_string(),
    ];
    let mut mounts = vec![workspace.clone()];
    for directory in run.directories {
        let directory = host_path(Path::new(directory))?;
        if directory != workspace {
            mounts.push(directory);
        }
    }
    for path in mounts {
        argv.push("--mount".to_string());
        argv.push(format!("type=bind,source={path},target={path}"));
    }
    for (source, target) in run.files {
        let source = host_path(source)?;
        argv.push("--mount".to_string());
        argv.push(format!(
            "type=bind,source={source},target={target},readonly"
        ));
    }
    for path in run.tmpfs {
        argv.extend(["--tmpfs".to_string(), path.clone()]);
    }
    argv.extend(["-w".to_string(), workspace]);
    for name in run.forwarded {
        argv.extend(["-e".to_string(), name.clone()]);
    }
    argv.push(run.image.to_string());
    argv.push(run.program.to_string());
    argv.extend(run.args.iter().cloned());
    Ok(argv)
}

/// The command that removes a container by name. A run calls it when fleetify stopped the
/// docker client, since stopping the client does not stop the container.
pub fn stop_argv(name: &str) -> Vec<String> {
    vec![
        docker_binary(),
        "rm".to_string(),
        "-f".to_string(),
        name.to_string(),
    ]
}

/// The path as text for a `--mount` value. Docker's option syntax cannot take a comma.
fn host_path(path: &Path) -> Result<String, String> {
    let text = path.display().to_string();
    if text.contains(',') {
        Err(format!(
            "path {text:?} contains a comma, which docker's mount syntax cannot take"
        ))
    } else {
        Ok(text)
    }
}

/// Refuses an image that is not available locally or was built for another agent.
/// Read-only: it only inspects the image.
pub fn check_image(image: &str, agent: &str) -> Result<(), String> {
    check_image_with(&docker_binary(), image, agent)
}

fn check_image_with(docker: &str, image: &str, agent: &str) -> Result<(), String> {
    check_name(image)?;
    let template = format!("{{{{index .Config.Labels \"{LABEL}\"}}}}");
    let output = Command::new(docker)
        .env_clear()
        .envs(client_vars())
        .args(["image", "inspect", "--format", &template, image])
        .output()
        .map_err(|err| format!("cannot run docker: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "image {image} is not available locally: build it or pull it first"
        ));
    }
    let label = String::from_utf8_lossy(&output.stdout).trim().to_string();
    match label.as_str() {
        "" | "<no value>" => Err(format!(
            "image {image} has no {LABEL} label: it is not a fleetify agent image"
        )),
        label if label == agent => Ok(()),
        other => Err(format!("image {image} is for agent {other}, not {agent}")),
    }
}

/// The variables a docker client needs, with their values from this process.
fn client_vars() -> Vec<(String, String)> {
    BASE_ENV
        .iter()
        .chain(CLIENT_ENV)
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_string(), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv_of(args: &[String], workspace: &str) -> Vec<String> {
        run_argv(&Run {
            image: "fleetify-agent-claude-code",
            name: "fleetify-test",
            program: "claude",
            args,
            workspace: Path::new(workspace),
            directories: &[],
            forwarded: &["ANTHROPIC_API_KEY".to_string()],
            files: &[],
            tmpfs: &[],
        })
        .unwrap()
    }

    #[test]
    fn the_workspace_is_mounted_at_its_host_path_and_is_the_working_directory() {
        let argv = argv_of(&["-p".to_string(), "do it".to_string()], "/home/u/ws");
        let mount = argv
            .iter()
            .position(|arg| arg == "--mount")
            .expect("a mount");
        assert_eq!(
            argv[mount + 1],
            "type=bind,source=/home/u/ws,target=/home/u/ws"
        );
        let workdir = argv.iter().position(|arg| arg == "-w").unwrap();
        assert_eq!(argv[workdir + 1], "/home/u/ws");
    }

    #[test]
    fn the_container_gets_names_not_values_and_the_prompt_last() {
        let argv = argv_of(
            &["-p".to_string(), "a prompt with $(id)".to_string()],
            "/ws",
        );
        let env = argv.iter().position(|arg| arg == "-e").unwrap();
        assert_eq!(argv[env + 1], "ANTHROPIC_API_KEY");
        let image = argv
            .iter()
            .position(|arg| arg == "fleetify-agent-claude-code")
            .unwrap();
        assert_eq!(&argv[image + 1..], ["claude", "-p", "a prompt with $(id)"]);
    }

    #[test]
    fn granted_directories_are_mounted_too_and_the_workspace_is_not_twice() {
        let dirs = ["/data/out".to_string(), "/ws".to_string()];
        let argv = run_argv(&Run {
            image: "img",
            name: "n",
            program: "claude",
            args: &[],
            workspace: Path::new("/ws"),
            directories: &dirs,
            forwarded: &[],
            files: &[],
            tmpfs: &[],
        })
        .unwrap();
        let mounts: Vec<&String> = argv
            .windows(2)
            .filter(|pair| pair[0] == "--mount")
            .map(|pair| &pair[1])
            .collect();
        assert_eq!(
            mounts,
            [
                "type=bind,source=/ws,target=/ws",
                "type=bind,source=/data/out,target=/data/out"
            ]
        );
    }

    #[test]
    fn names_and_paths_that_could_split_the_command_are_refused() {
        assert!(check_name("--privileged").is_err());
        assert!(check_name("a b").is_err());
        assert!(check_name("").is_err());
        assert!(check_name("registry.example.com:5000/fleetify/agent:1.0").is_ok());
        assert!(host_path(Path::new("/a,b")).is_err());
    }

    #[test]
    fn the_stop_command_removes_the_named_container() {
        let argv = stop_argv("fleetify-x");
        assert_eq!(&argv[1..], ["rm", "-f", "fleetify-x"]);
    }

    #[cfg(unix)]
    fn fake_docker(name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("fleetify-container-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn an_image_is_accepted_only_for_the_agent_on_its_label() {
        let right = fake_docker("right", "echo claude-code");
        let wrong = fake_docker("wrong", "echo opencode");
        let unlabelled = fake_docker("unlabelled", "echo '<no value>'");
        let missing = fake_docker("missing", "echo 'No such image' >&2; exit 1");
        let ok = |bin: &Path| check_image_with(bin.to_str().unwrap(), "img", "claude-code");
        assert!(ok(&right).is_ok());
        assert!(ok(&wrong).unwrap_err().contains("is for agent opencode"));
        assert!(
            ok(&unlabelled)
                .unwrap_err()
                .contains("no io.fleetify.agent label")
        );
        assert!(ok(&missing).unwrap_err().contains("not available locally"));
        let _ = std::fs::remove_dir_all(right.parent().unwrap());
    }
}
