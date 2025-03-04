//! Implement a wrapper around setup and tear-down of Direnv-based test
//! cases.

use lorri::build_loop::BuildLoop;
use lorri::builder;
use lorri::builder::BuildError;
use lorri::cas::ContentAddressable;
use lorri::nix::options::NixOptions;
use lorri::ops;
use lorri::project;
use lorri::project::Project;
use lorri::AbsPathBuf;
use lorri::NixFile;

use std::collections::HashMap;
use std::fs::File;
use std::iter::FromIterator;
use std::path::PathBuf;
use std::process::Command;
use tempfile::{tempdir, TempDir};

pub struct DirenvTestCase {
    projectdir: TempDir,
    // only kept around to not delete tempdir
    #[allow(dead_code)]
    pub cachedir: TempDir,
    project: Project,
    logger: slog::Logger,
}

impl DirenvTestCase {
    pub fn new(name: &str) -> DirenvTestCase {
        let projectdir = tempdir().expect("tempfile::tempdir() failed us!");
        let cachedir_tmp = tempdir().expect("tempfile::tempdir() failed us!");
        let cachedir = AbsPathBuf::new(cachedir_tmp.path().to_owned()).unwrap();

        let test_root =
            PathBuf::from_iter(&[env!("CARGO_MANIFEST_DIR"), "tests", "integration", name]);

        let shell_file = NixFile::from(AbsPathBuf::new(test_root.join("shell.nix")).unwrap());

        let cas = ContentAddressable::new(cachedir.join("cas").to_owned()).unwrap();
        let project = Project::new(shell_file.clone(), &cachedir.join("gc_roots"), cas).unwrap();

        DirenvTestCase {
            projectdir,
            cachedir: cachedir_tmp,
            project,
            logger: lorri::logging::test_logger("direnv_test_case"),
        }
    }

    /// Execute the build loop one time
    pub fn evaluate(&mut self) -> Result<builder::OutputPath<project::RootPath>, BuildError> {
        BuildLoop::new(&self.project, NixOptions::empty(), self.logger.clone())
            .expect("could not set up build loop")
            .once()
    }

    /// Run `direnv allow` and then `direnv export json`, and return
    /// the environment DirEnv would produce.
    pub fn get_direnv_variables(&self) -> DirenvEnv {
        let envrc = File::create(self.projectdir.path().join(".envrc")).unwrap();
        let paths = lorri::ops::get_paths().unwrap();
        ops::op_direnv(self.project.clone(), &paths, envrc, &self.logger).unwrap();

        {
            let mut allow = self.direnv_cmd();
            allow.arg("allow");
            let result = allow.status().expect("Failed to run direnv allow");
            assert!(result.success());
        }

        let mut env = self.direnv_cmd();
        env.args(["export", "json"]);
        let result = env.output().expect("Failed to run direnv export json");
        if !result.status.success() {
            println!("stderr: {}", String::from_utf8_lossy(&result.stderr));
            println!("\n\n\nstdout: {}", String::from_utf8_lossy(&result.stdout));
        }
        assert!(result.status.success());

        serde_json::from_slice(&result.stdout).unwrap()
    }

    fn direnv_cmd(&self) -> Command {
        let mut d = Command::new("direnv");
        // From: https://github.com/direnv/direnv/blob/1423e495c54de3adafde8e26218908010c955514/test/direnv-test.bash
        d.env_remove("DIRENV_BASH");
        d.env_remove("DIRENV_DIR");
        d.env_remove("DIRENV_MTIME");
        d.env_remove("DIRENV_WATCHES");
        d.env_remove("DIRENV_DIFF");
        d.env("DIRENV_CONFIG", self.projectdir.path());
        d.env("XDG_CONFIG_HOME", self.projectdir.path());
        d.current_dir(self.projectdir.path());

        d
    }
}

/// The resulting environment Direnv after running Direnv. Note:
/// Direnv returns `{ "varname": null, "varname": "something" }`
/// so the value type is `Option<String>`. This makes `.get()`
/// operations clunky, so be prepared to check for `Some(None)` and
/// `Some(Some("val"))`.
#[derive(Deserialize, Debug)]
pub struct DirenvEnv(HashMap<String, Option<String>>);

impl DirenvEnv {
    /// Get an environment value with a borrowed str in the deepest Option.
    /// Makes asserts nicer, like:
    ///
    ///    assert!(env.get_env("foo"), Value("bar"));
    pub fn get_env(&self, key: &str) -> DirenvValue {
        match self.0.get(key) {
            Some(Some(val)) => DirenvValue::Value(val),
            Some(None) => DirenvValue::Unset,
            None => DirenvValue::NotSet,
        }
    }

    /// Like HashMap.retain, but don’t mutate self.
    pub fn retain<F>(&self, f: F) -> HashMap<String, Option<String>>
    where
        F: Fn(&str) -> bool,
    {
        let mut new = self.0.to_owned();
        new.retain(|k, _| f(k));
        new
    }
}

/// Environemnt Values from Direnv
#[derive(Debug, PartialEq)]
pub enum DirenvValue<'a> {
    /// This variable will not be modified.
    NotSet,

    /// This variable will be unset when entering direnv.
    Unset,

    /// This variable will be set to exactly.
    Value(&'a str),
}
