use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use yamlpath::{Document, QueryError};

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
}

pub fn workflow_paths(input: &Path) -> Result<Vec<PathBuf>, LoadError> {
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
        if is_yaml && path.is_file() {
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
        let paths = workflow_paths(Path::new(FIXTURES)).unwrap();
        let names: Vec<_> = paths
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["Deploy.YML", "broken.yml", "ci.yml", "release.yaml"]
        );
    }

    #[test]
    fn directory_without_yaml_files_is_an_error() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/no-workflows");
        let result = workflow_paths(&path);
        assert!(matches!(result, Err(LoadError::NoWorkflows { .. })));
    }

    #[test]
    fn file_path_is_returned_as_is_whatever_its_extension() {
        let path = Path::new(FIXTURES).join("notes.txt");
        assert_eq!(workflow_paths(&path).unwrap(), [path]);
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
