//! Isolation backends: how an agent run is put into its own environment (stage 3).
//!
//! A backend turns a run into a command line that the runner starts, plus the command that
//! stops whatever that command started. The runner and the agent adapters do not know which
//! backend is in use. Docker is the only one built. A backend that starts a virtual machine
//! (Firecracker) or a system container (LXC) implements the same trait, so only its own module
//! changes.

use std::path::{Path, PathBuf};

use crate::container;
use crate::runner::EnvPolicy;

/// The backend that runs containers through the docker client. The default.
pub const DOCKER: &str = "docker";
/// Every backend this build knows. A recipe that names another one is refused.
pub const BACKENDS: &[&str] = &[DOCKER];

/// One agent run to isolate.
pub struct Spec<'a> {
    /// The image or root filesystem the run starts from. Its meaning is the backend's.
    pub image: &'a str,
    /// The program inside the environment, e.g. `claude`.
    pub program: &'a str,
    /// The arguments after the program. The prompt is among them.
    pub args: &'a [String],
    /// The workspace. It is visible at the same path inside.
    pub workspace: &'a Path,
    /// Granted directories, visible at the same path inside.
    pub directories: &'a [String],
    /// Variable names copied from the environment that starts the run.
    pub forwarded: &'a [String],
    /// Variables with fixed values.
    pub set: &'a [(String, String)],
    /// Host files visible read-only at the given path inside. Missing files are left out
    /// by the caller.
    pub files: &'a [(PathBuf, String)],
    /// Paths inside that get a fresh, empty, writable area.
    pub tmpfs: &'a [String],
}

/// What the runner needs to start one isolated run.
pub struct Launch {
    /// The command line to start. Its first element is the program that starts the run.
    pub argv: Vec<String>,
    /// The environment of that command.
    pub env: EnvPolicy,
    /// The command that stops what the run started outside its own process tree.
    pub stop: Vec<String>,
}

/// One way of isolating an agent run.
pub trait Backend {
    /// Refuses an image that the backend cannot start, or that belongs to another agent.
    fn check_image(&self, image: &str, agent: &str) -> Result<(), String>;
    /// The command line for one run. Starts nothing itself.
    fn launch(&self, spec: &Spec) -> Result<Launch, String>;
}

/// The backend with this name. Refuses names this build does not know.
pub fn backend(name: &str) -> Result<Box<dyn Backend>, String> {
    match name {
        DOCKER => Ok(Box::new(Docker)),
        other => Err(format!(
            "isolation {other:?} is not supported in this build; only {} is",
            BACKENDS.join(", ")
        )),
    }
}

/// The docker backend, implemented in `container.rs`.
pub struct Docker;

impl Backend for Docker {
    fn check_image(&self, image: &str, agent: &str) -> Result<(), String> {
        container::check_image(image, agent)
    }

    fn launch(&self, spec: &Spec) -> Result<Launch, String> {
        let name = container::container_name();
        let argv = container::run_argv(&container::Run {
            image: spec.image,
            name: &name,
            program: spec.program,
            args: spec.args,
            workspace: spec.workspace,
            directories: spec.directories,
            forwarded: spec.forwarded,
            files: spec.files,
            tmpfs: spec.tmpfs,
        })?;
        Ok(Launch {
            argv,
            env: container::client_env(spec.forwarded.to_vec(), spec.set.to_vec()),
            stop: container::stop_argv(&name),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_is_the_only_backend_and_others_are_refused_by_name() {
        assert!(backend(DOCKER).is_ok());
        let err = backend("firecracker").err().unwrap();
        assert!(
            err.contains("firecracker") && err.contains("only docker"),
            "{err}"
        );
    }

    #[test]
    fn the_docker_launch_keeps_names_and_puts_the_program_last() {
        let args = ["-p".to_string(), "prompt".to_string()];
        let forwarded = ["ANTHROPIC_API_KEY".to_string()];
        let set = [("OPENCODE_CONFIG_CONTENT".to_string(), "{}".to_string())];
        let launch = backend(DOCKER)
            .unwrap()
            .launch(&Spec {
                image: "fleetify-agent-claude-code",
                program: "claude",
                args: &args,
                workspace: Path::new("/ws"),
                directories: &[],
                forwarded: &forwarded,
                set: &set,
                files: &[],
                tmpfs: &[],
            })
            .unwrap();
        assert_eq!(launch.argv[0], "docker");
        assert_eq!(&launch.argv[launch.argv.len() - 2..], ["-p", "prompt"]);
        assert_eq!(launch.stop[1..3], ["rm".to_string(), "-f".to_string()]);
        let EnvPolicy::Allowlist { names, set } = launch.env else {
            panic!("the client gets an allowlist");
        };
        assert!(names.contains(&"ANTHROPIC_API_KEY".to_string()));
        assert_eq!(set.len(), 1);
    }
}
