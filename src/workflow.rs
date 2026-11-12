use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use yamlpath::{Document, QueryError, Route};

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("failed to read {}: {source}", path.display())]
    Read { path: PathBuf, source: io::Error },
    #[error("{} is not a valid workflow: {source}", path.display())]
    Parse {
        path: PathBuf,
        source: yaml_serde::Error,
    },
    #[error("failed to index {}: {source}", path.display())]
    Index { path: PathBuf, source: QueryError },
    #[error("no .yml or .yaml files directly inside {}", path.display())]
    NoWorkflows { path: PathBuf },
}

const WORKFLOW_EXTENSIONS: [&str; 2] = ["yml", "yaml"];

pub struct Workflow {
    pub path: PathBuf,
    pub model: github_actions_models::workflow::Workflow,
    pub document: Document,
}

impl Workflow {
    pub fn load(path: &Path) -> Result<Self, LoadError> {
        let source = fs::read_to_string(path).map_err(|source| LoadError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(path, source)
    }

    pub fn parse(path: &Path, source: String) -> Result<Self, LoadError> {
        let model = yaml_serde::from_str(&source).map_err(|source| LoadError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        let document = Document::new(source).map_err(|source| LoadError::Index {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self {
            path: path.to_path_buf(),
            model,
            document,
        })
    }

    pub fn position(&self, route: &Route) -> Option<(usize, usize)> {
        let feature = self.document.query_pretty(route).ok()?;
        let (row, column) = feature.location.point_span.0;
        Some((row + 1, column + 1))
    }
}

pub fn workflow_paths(inputs: &[PathBuf]) -> Vec<Result<PathBuf, LoadError>> {
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for input in inputs {
        match paths_in(input) {
            Ok(paths) => found.extend(
                paths
                    .into_iter()
                    .filter(|path| seen.insert(fs::canonicalize(path).unwrap_or(path.clone())))
                    .map(Ok),
            ),
            Err(error) => found.push(Err(error)),
        }
    }
    found
}

fn paths_in(input: &Path) -> Result<Vec<PathBuf>, LoadError> {
    if !input.is_dir() {
        return Ok(vec![input.to_path_buf()]);
    }
    let read_error = |source| LoadError::Read {
        path: input.to_path_buf(),
        source,
    };
    let mut paths = Vec::new();
    for entry in fs::read_dir(input).map_err(read_error)? {
        let path = entry.map_err(read_error)?.path();
        let is_yaml = path.extension().is_some_and(|extension| {
            WORKFLOW_EXTENSIONS
                .iter()
                .any(|yaml| extension.eq_ignore_ascii_case(yaml))
        });
        let is_file_or_unresolvable =
            fs::metadata(&path).map_or(true, |metadata| metadata.is_file());
        if is_yaml && is_file_or_unresolvable {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        return Err(LoadError::NoWorkflows {
            path: input.to_path_buf(),
        });
    }
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/workflows");

    #[test]
    fn directory_lists_only_yaml_files_directly_inside_in_name_order() {
        let paths = paths_in(Path::new(FIXTURES)).unwrap();
        let names: Vec<_> = paths
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["Deploy.YML", "broken.yml", "ci.yml", "release.yaml"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_is_listed_so_that_loading_reports_it() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("agent.yml");
        std::os::unix::fs::symlink(dir.path().join("missing.yml"), &link).unwrap();

        assert_eq!(paths_in(dir.path()).unwrap(), std::slice::from_ref(&link));
        assert!(matches!(Workflow::load(&link), Err(LoadError::Read { .. })));
    }

    #[cfg(unix)]
    #[test]
    fn named_pipe_is_not_listed_so_that_reading_does_not_block() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("ci.yml"), "").unwrap();
        let status = std::process::Command::new("mkfifo")
            .arg(dir.path().join("pipe.yml"))
            .status()
            .unwrap();
        assert!(status.success());

        assert_eq!(paths_in(dir.path()).unwrap(), [dir.path().join("ci.yml")]);
    }

    #[test]
    fn directory_without_yaml_files_is_an_error() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/no-workflows");
        assert!(matches!(
            paths_in(&path),
            Err(LoadError::NoWorkflows { .. })
        ));
    }

    #[test]
    fn file_path_is_returned_as_is_whatever_its_extension() {
        let path = Path::new(FIXTURES).join("notes.txt");
        assert_eq!(paths_in(&path).unwrap(), [path]);
    }

    #[test]
    fn inputs_are_expanded_in_order_and_a_file_reached_twice_comes_once() {
        let fixtures = Path::new(FIXTURES);
        let inputs = [
            fixtures.join("release.yaml"),
            fixtures.join("nested/../ci.yml"),
            fixtures.to_path_buf(),
        ];
        let names: Vec<_> = workflow_paths(&inputs)
            .into_iter()
            .map(|path| {
                path.unwrap()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            names,
            ["release.yaml", "ci.yml", "Deploy.YML", "broken.yml"]
        );
    }

    #[test]
    fn an_input_that_cannot_be_expanded_keeps_its_place_among_the_results() {
        let fixtures = Path::new(FIXTURES);
        let inputs = [
            fixtures.join("ci.yml"),
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/no-workflows"),
            fixtures.join("release.yaml"),
        ];
        let results = workflow_paths(&inputs);
        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(matches!(results[1], Err(LoadError::NoWorkflows { .. })));
        assert!(results[2].is_ok());
    }

    #[test]
    fn loads_a_workflow_file() {
        let workflow = Workflow::load(&Path::new(FIXTURES).join("ci.yml")).unwrap();
        assert!(workflow.model.jobs.contains_key("test"));
    }

    #[test]
    fn missing_file_is_a_read_error() {
        let result = Workflow::load(&Path::new(FIXTURES).join("missing.yml"));
        assert!(matches!(result, Err(LoadError::Read { .. })));
    }

    #[test]
    fn yaml_that_is_not_a_workflow_is_a_parse_error() {
        let result = Workflow::load(&Path::new(FIXTURES).join("broken.yml"));
        assert!(matches!(result, Err(LoadError::Parse { .. })));
    }
}
