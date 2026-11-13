use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::{env, fs, io};

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
const WORKSPACE_VARIABLE: &str = "GITHUB_WORKSPACE";
const REPOSITORY_MARKER: &str = ".git";
const VERBATIM_PREFIX: &str = r"\\?\";

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
        let (row, byte_column) = feature.location.point_span.0;
        let offset = feature.location.byte_span.0;
        let line_before = &self.document.source()[offset - byte_column..offset];
        Some((row + 1, line_before.encode_utf16().count() + 1))
    }
}

pub enum RepositoryPath {
    Relative(PathBuf),
    Absolute(PathBuf),
}

pub fn repository_path(path: &Path) -> RepositoryPath {
    let resolved =
        fs::canonicalize(path).map_or_else(|_| path.to_path_buf(), without_verbatim_prefix);
    let workspace = env::var_os(WORKSPACE_VARIABLE)
        .and_then(|workspace| fs::canonicalize(workspace).ok())
        .map(without_verbatim_prefix)
        .filter(|workspace| resolved.starts_with(workspace));
    let root = workspace.or_else(|| {
        resolved
            .ancestors()
            .skip(1)
            .find(|directory| directory.join(REPOSITORY_MARKER).exists())
            .map(Path::to_path_buf)
    });
    let relative = match &root {
        Some(root) => resolved.strip_prefix(root).ok(),
        None => path.is_relative().then_some(path),
    };
    let folded = relative.map(|relative| {
        let mut parts: Vec<Component> = Vec::new();
        for component in relative.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir if matches!(parts.last(), Some(Component::Normal(_))) => {
                    parts.pop();
                }
                component => parts.push(component),
            }
        }
        parts.into_iter().collect::<PathBuf>()
    });
    match folded {
        Some(folded) if !folded.starts_with(Component::ParentDir) => {
            RepositoryPath::Relative(folded)
        }
        Some(_) | None => RepositoryPath::Absolute(resolved),
    }
}

fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    match path
        .to_str()
        .and_then(|text| text.strip_prefix(VERBATIM_PREFIX))
    {
        Some(stripped) => PathBuf::from(stripped),
        None => path,
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

    #[test]
    fn columns_count_utf16_code_units() {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            "on: issues\njobs:\n  a:\n    runs-on: ubuntu-latest\n    steps:\n      - {name: \"日本語\", uses: x/y@v1, with: {b: c}}\n"
                .to_string(),
        )
        .unwrap();
        let route = yamlpath::route!["jobs", "a", "steps", 0, "with", "b"];
        assert_eq!(workflow.position(&route), Some((6, 43)));
    }

    #[test]
    fn repository_paths_start_at_the_nearest_git_directory() {
        let repository = tempfile::tempdir().unwrap();
        fs::create_dir(repository.path().join(REPOSITORY_MARKER)).unwrap();
        let workflows = repository.path().join(".github/workflows");
        fs::create_dir_all(&workflows).unwrap();
        let file = workflows.join("a.yml");
        fs::write(&file, "").unwrap();
        for path in [file.clone(), workflows.join("../workflows/./a.yml")] {
            assert!(matches!(
                repository_path(&path),
                RepositoryPath::Relative(relative) if relative == Path::new(".github/workflows/a.yml")
            ));
        }
        let outside = tempfile::tempdir().unwrap();
        let loose = outside.path().join("a.yml");
        fs::write(&loose, "").unwrap();
        assert!(matches!(
            repository_path(&loose),
            RepositoryPath::Absolute(_)
        ));
    }
}
