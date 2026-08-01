//! Offline authoring tools for versioned Frust application bundles.
//!
//! The SDK deliberately reuses [`frust_kernel::app::Manifest`] and its
//! validator. The CLI therefore cannot accept a manifest the kernel would
//! reject merely because a second schema implementation drifted.

use std::collections::BTreeSet;
use std::fmt::{self, Display};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub use frust_kernel::app::{
    ExtensionDecl, Manifest, RouteDecl, ScriptDecl, MANIFEST_VERSION, RESERVED_ROUTE_PATHS,
};
pub use frust_kernel::sync::{AggregateDef, DocTypeDef, FetchFrom, FieldDef, MetricSpec, Rule};
pub use frust_kernel::workflow::{StateDef, StateRule, TransitionDef, WorkflowDef};

pub const BUNDLE_FORMAT_VERSION: u32 = 1;
pub const MANIFEST_FILE: &str = "frust-app.json";
pub const WIT_FILE: &str = "wit/plugin.wit";

const CANONICAL_WIT: &str = include_str!("../../wit/plugin.wit");
const TEMPLATE_MANIFEST: &str = include_str!("../templates/rust-hook/frust-app.json");
const TEMPLATE_CARGO: &str = include_str!("../templates/rust-hook/Cargo.toml");
const TEMPLATE_LIB: &str = include_str!("../templates/rust-hook/src/lib.rs");
const TEMPLATE_README: &str = include_str!("../templates/rust-hook/README.md");
const TEMPLATE_GITIGNORE: &str = include_str!("../templates/rust-hook/gitignore");

#[derive(Debug)]
pub enum Error {
    Io { path: PathBuf, source: io::Error },
    InvalidManifest(String),
    InvalidProject(Vec<String>),
    TargetExists(PathBuf),
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::InvalidManifest(message) => write!(f, "{message}"),
            Self::InvalidProject(problems) => {
                write!(f, "project check failed:\n- {}", problems.join("\n- "))
            }
            Self::TargetExists(path) => {
                write!(f, "refusing to overwrite existing path {}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

fn io_at(path: &Path, source: io::Error) -> Error {
    Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Typed construction that still passes through the kernel's authoritative
/// validation before yielding a manifest.
#[derive(Debug, Clone)]
pub struct ManifestBuilder {
    manifest: Manifest,
}

impl ManifestBuilder {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            manifest: Manifest {
                manifest_version: MANIFEST_VERSION,
                name: name.into(),
                version: version.into(),
                label: String::new(),
                doctypes: Vec::new(),
                client_scripts: Vec::new(),
                server_scripts: Vec::new(),
                routes: Vec::new(),
                components: Vec::new(),
                workflows: Vec::new(),
                extends: Vec::new(),
            },
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.manifest.label = label.into();
        self
    }

    pub fn doctype(mut self, mut doctype: DocTypeDef) -> Self {
        if doctype.app.is_none() {
            doctype.app = Some(self.manifest.name.clone());
        }
        self.manifest.doctypes.push(doctype);
        self
    }

    pub fn client_script(mut self, doctype: impl Into<String>, script: impl Into<String>) -> Self {
        self.manifest.client_scripts.push(ScriptDecl {
            doctype: doctype.into(),
            hook: "validate".to_string(),
            script: script.into(),
        });
        self
    }

    pub fn server_script(
        mut self,
        doctype: impl Into<String>,
        hook: impl Into<String>,
        script: impl Into<String>,
    ) -> Self {
        self.manifest.server_scripts.push(ScriptDecl {
            doctype: doctype.into(),
            hook: hook.into(),
            script: script.into(),
        });
        self
    }

    pub fn component(mut self, filename: impl Into<String>) -> Self {
        self.manifest.components.push(filename.into());
        self
    }

    pub fn route(mut self, path: impl Into<String>, component: impl Into<String>) -> Self {
        self.manifest.routes.push(RouteDecl {
            path: path.into(),
            component: component.into(),
        });
        self
    }

    pub fn workflow(mut self, workflow: WorkflowDef) -> Self {
        self.manifest.workflows.push(workflow);
        self
    }

    pub fn extension(mut self, extension: ExtensionDecl) -> Self {
        self.manifest.extends.push(extension);
        self
    }

    pub fn build(self) -> std::result::Result<Manifest, Vec<String>> {
        let problems = self.manifest.validate();
        if problems.is_empty() {
            Ok(self.manifest)
        } else {
            Err(problems)
        }
    }
}

/// A DocType value with all optional contract fields initialized safely.
pub fn doctype(name: impl Into<String>, fields: Vec<FieldDef>) -> DocTypeDef {
    DocTypeDef {
        name: name.into(),
        app: None,
        submittable: false,
        fields,
        aggregates: Vec::new(),
    }
}

/// A field value with all optional contract fields initialized safely.
pub fn field(name: impl Into<String>, fieldtype: impl Into<String>) -> FieldDef {
    FieldDef {
        fieldname: name.into(),
        fieldtype: fieldtype.into(),
        required: false,
        options: Vec::new(),
        child_storage: None,
        depends_on: None,
        read_only_when: None,
        required_when: None,
        invalid_when: None,
        fetch_from: None,
    }
}

#[derive(Debug, Clone)]
pub struct CheckedProject {
    pub root: PathBuf,
    pub manifest: Manifest,
    pub components: Vec<PathBuf>,
    pub wit_sha256: String,
}

/// Create the Rust hook template. Every generated file is either app-owned or
/// an exact, checksum-verifiable copy of the SDK's canonical WIT contract.
pub fn create_project(target: &Path, name: &str, label: Option<&str>) -> Result<PathBuf> {
    if target.exists() {
        return Err(Error::TargetExists(target.to_path_buf()));
    }
    let _ = ManifestBuilder::new(name, "0.1.0")
        .label(label.unwrap_or(name))
        .build()
        .map_err(Error::InvalidProject)?;

    for dir in [target.to_path_buf(), target.join("src"), target.join("wit")] {
        fs::create_dir_all(&dir).map_err(|e| io_at(&dir, e))?;
    }

    let replacements = |text: &str| {
        text.replace("{{app_name}}", name)
            .replace("{{app_label}}", label.unwrap_or(name))
    };
    write_new(
        &target.join(MANIFEST_FILE),
        replacements(TEMPLATE_MANIFEST).as_bytes(),
    )?;
    write_new(
        &target.join("Cargo.toml"),
        replacements(TEMPLATE_CARGO).as_bytes(),
    )?;
    write_new(
        &target.join("src/lib.rs"),
        replacements(TEMPLATE_LIB).as_bytes(),
    )?;
    write_new(
        &target.join("README.md"),
        replacements(TEMPLATE_README).as_bytes(),
    )?;
    write_new(&target.join(".gitignore"), TEMPLATE_GITIGNORE.as_bytes())?;
    write_new(
        &target.join(WIT_FILE),
        normalize_newlines(CANONICAL_WIT).as_bytes(),
    )?;
    Ok(target.to_path_buf())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        return Err(Error::TargetExists(path.to_path_buf()));
    }
    fs::write(path, bytes).map_err(|e| io_at(path, e))
}

/// Run every check available without a kernel or database: the kernel's own
/// manifest validator, project ownership, canonical WIT and Wasm decoding.
pub fn check_project(root: &Path) -> Result<CheckedProject> {
    let manifest_path = root.join(MANIFEST_FILE);
    let text = fs::read_to_string(&manifest_path).map_err(|e| io_at(&manifest_path, e))?;
    let manifest = Manifest::parse(&text).map_err(|e| Error::InvalidManifest(e.to_string()))?;
    let mut problems = manifest.validate();

    for doctype in &manifest.doctypes {
        if doctype.app.as_deref() != Some(manifest.name.as_str()) {
            problems.push(format!(
                "doctype '{}' must set app to '{}'",
                doctype.name, manifest.name
            ));
        }
    }

    let mut component_names = BTreeSet::new();
    let mut components = Vec::new();
    for name in &manifest.components {
        if !component_names.insert(name) {
            problems.push(format!("component '{name}' is declared twice"));
            continue;
        }
        let path = root.join("components").join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                problems.push(format!("component '{name}' must not be a symbolic link"));
            }
            Ok(metadata) if metadata.is_file() => {
                let mut config = wasmtime::Config::new();
                config.wasm_component_model(true);
                match wasmtime::Engine::new(&config)
                    .and_then(|engine| wasmtime::component::Component::from_file(&engine, &path))
                {
                    Ok(_) => components.push(path),
                    Err(error) => problems.push(format!(
                        "component '{}' is not a valid WebAssembly component: {error}",
                        path.display()
                    )),
                }
            }
            Ok(_) => problems.push(format!(
                "component '{}' is not a regular file",
                path.display()
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                problems.push(format!(
                    "declared component '{}' is missing",
                    path.display()
                ));
            }
            Err(error) => return Err(io_at(&path, error)),
        }
    }

    let wit_path = root.join(WIT_FILE);
    match fs::read_to_string(&wit_path) {
        Ok(wit) if normalize_newlines(&wit) == normalize_newlines(CANONICAL_WIT) => {}
        Ok(_) => problems.push(format!(
            "{} does not match the canonical Frust WIT contract (expected sha256 {})",
            wit_path.display(),
            canonical_wit_sha256()
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => problems.push(format!(
            "{} is missing; regenerate it with the matching frust-app SDK",
            wit_path.display()
        )),
        Err(error) => return Err(io_at(&wit_path, error)),
    }

    if !problems.is_empty() {
        return Err(Error::InvalidProject(problems));
    }

    components.sort();
    Ok(CheckedProject {
        root: root.to_path_buf(),
        manifest,
        components,
        wit_sha256: canonical_wit_sha256(),
    })
}

#[derive(Debug, Clone, Serialize)]
struct BundleMetadata<'a> {
    bundle_format: u32,
    app: &'a str,
    version: &'a str,
    manifest_version: i64,
    wit_sha256: &'a str,
}

#[derive(Debug, Clone)]
pub struct PackReport {
    pub output: PathBuf,
    pub sha256: String,
    pub files: usize,
}

/// Create an uncompressed deterministic tar bundle. Entries are sorted and
/// carry fixed ownership, permissions and timestamps, so identical inputs
/// produce identical bytes across invocations.
pub fn pack_project(root: &Path, output: &Path) -> Result<PackReport> {
    if output.exists() {
        return Err(Error::TargetExists(output.to_path_buf()));
    }
    let checked = check_project(root)?;
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| io_at(parent, e))?;

    let canonical_manifest = serde_json::to_vec_pretty(&checked.manifest)
        .map_err(|e| Error::InvalidManifest(format!("cannot serialize manifest: {e}")))?;
    let metadata = serde_json::to_vec_pretty(&BundleMetadata {
        bundle_format: BUNDLE_FORMAT_VERSION,
        app: &checked.manifest.name,
        version: &checked.manifest.version,
        manifest_version: checked.manifest.manifest_version,
        wit_sha256: &checked.wit_sha256,
    })
    .map_err(|e| Error::InvalidManifest(format!("cannot serialize bundle metadata: {e}")))?;

    let mut files: Vec<(String, Vec<u8>)> = vec![
        ("bundle.json".to_string(), with_final_newline(metadata)),
        (
            "manifest.json".to_string(),
            with_final_newline(canonical_manifest),
        ),
        (
            WIT_FILE.to_string(),
            normalize_newlines(CANONICAL_WIT).into_bytes(),
        ),
    ];
    for component in &checked.components {
        let name = component
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                Error::InvalidProject(vec![format!(
                    "component path is not UTF-8: {}",
                    component.display()
                )])
            })?;
        let bytes = fs::read(component).map_err(|e| io_at(component, e))?;
        files.push((format!("components/{name}"), bytes));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut checksums = String::new();
    for (path, bytes) in &files {
        checksums.push_str(&format!("{}  {path}\n", sha256_bytes(bytes)));
    }
    files.push(("checksums.sha256".to_string(), checksums.into_bytes()));

    let temporary = output.with_extension("frust.tmp");
    let result = (|| -> Result<()> {
        let mut file = fs::File::create(&temporary).map_err(|e| io_at(&temporary, e))?;
        write_tar(&mut file, &files).map_err(|e| io_at(&temporary, e))?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    fs::rename(&temporary, output).map_err(|e| io_at(output, e))?;
    let archive_bytes = fs::read(output).map_err(|e| io_at(output, e))?;
    Ok(PackReport {
        output: output.to_path_buf(),
        sha256: sha256_bytes(&archive_bytes),
        files: files.len(),
    })
}

fn write_tar<W: Write>(writer: &mut W, files: &[(String, Vec<u8>)]) -> io::Result<()> {
    for (path, bytes) in files {
        let name = path.as_bytes();
        if !path.is_ascii() || name.len() > 100 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("bundle path is not USTAR-compatible: {path}"),
            ));
        }

        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name);
        write_octal(&mut header[100..108], 0o644)?;
        write_octal(&mut header[108..116], 0)?;
        write_octal(&mut header[116..124], 0)?;
        write_octal(&mut header[124..136], bytes.len() as u64)?;
        write_octal(&mut header[136..148], 0)?;
        header[148..156].fill(b' ');
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
        let rendered = format!("{checksum:06o}\0 ");
        header[148..156].copy_from_slice(rendered.as_bytes());

        writer.write_all(&header)?;
        writer.write_all(bytes)?;
        let padding = (512 - bytes.len() % 512) % 512;
        if padding != 0 {
            writer.write_all(&vec![0; padding])?;
        }
    }
    writer.write_all(&[0; 1024])
}

fn write_octal(field: &mut [u8], value: u64) -> io::Result<()> {
    let rendered = format!("{:0width$o}\0", value, width = field.len() - 1);
    if rendered.len() != field.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "bundle entry is too large for a USTAR header",
        ));
    }
    field.copy_from_slice(rendered.as_bytes());
    Ok(())
}

pub fn canonical_wit_sha256() -> String {
    sha256_bytes(normalize_newlines(CANONICAL_WIT).as_bytes())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn with_final_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    if !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "frust-app-sdk-{label}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn typed_builder_uses_the_kernel_validator() {
        let invoice = doctype(
            "sales_invoice",
            vec![field("customer", "Data"), field("total", "Currency")],
        );
        let manifest = ManifestBuilder::new("sales", "1.2.0")
            .label("Sales")
            .doctype(invoice)
            .build()
            .expect("valid manifest");
        assert_eq!(manifest.manifest_version, MANIFEST_VERSION);
        assert_eq!(manifest.doctypes[0].app.as_deref(), Some("sales"));

        let errors = ManifestBuilder::new("bad-name", "1")
            .build()
            .expect_err("kernel rules must be enforced");
        assert!(errors.iter().any(|e| e.contains("identifier")));
        assert!(errors.iter().any(|e| e.contains("MAJOR.MINOR.PATCH")));
    }

    #[test]
    fn scaffold_is_immediately_checkable_and_pins_canonical_wit() {
        let temp = TempDir::new("new");
        let project = temp.0.join("ledger");
        create_project(&project, "ledger", Some("Ledger")).unwrap();

        let checked = check_project(&project).unwrap();
        assert_eq!(checked.manifest.name, "ledger");
        assert_eq!(checked.wit_sha256, canonical_wit_sha256());
        assert!(project.join("src/lib.rs").is_file());
    }

    #[test]
    fn check_reports_manifest_artifact_and_wit_problems_together() {
        let temp = TempDir::new("problems");
        let project = temp.0.join("broken");
        create_project(&project, "broken", None).unwrap();
        fs::write(
            project.join(MANIFEST_FILE),
            r#"{
                "manifest_version": 99,
                "name": "broken",
                "version": "nope",
                "doctypes": [],
                "routes": [{"path":"report","component":"missing.wasm"}],
                "components": ["missing.wasm"]
            }"#,
        )
        .unwrap();
        fs::write(project.join(WIT_FILE), "stale").unwrap();

        let Error::InvalidProject(problems) = check_project(&project).unwrap_err() else {
            panic!("expected project errors");
        };
        let joined = problems.join("\n");
        assert!(joined.contains("manifest_version"), "{joined}");
        assert!(joined.contains("MAJOR.MINOR.PATCH"), "{joined}");
        assert!(joined.contains("missing"), "{joined}");
        assert!(joined.contains("canonical Frust WIT"), "{joined}");
    }

    #[test]
    fn pack_is_byte_for_byte_deterministic_and_self_describing() {
        let temp = TempDir::new("pack");
        let project = temp.0.join("inventory");
        create_project(&project, "inventory", Some("Inventory")).unwrap();
        let first = temp.0.join("first.frust");
        let second = temp.0.join("second.frust");

        let a = pack_project(&project, &first).unwrap();
        let b = pack_project(&project, &second).unwrap();
        assert_eq!(a.sha256, b.sha256);
        assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());

        let names = tar_entry_names(&fs::read(first).unwrap());
        assert_eq!(
            names,
            [
                "bundle.json",
                "manifest.json",
                "wit/plugin.wit",
                "checksums.sha256"
            ]
        );
    }

    fn tar_entry_names(bytes: &[u8]) -> Vec<String> {
        let mut names = Vec::new();
        let mut offset = 0;
        while offset + 512 <= bytes.len() && bytes[offset..offset + 512].iter().any(|b| *b != 0) {
            let header = &bytes[offset..offset + 512];
            let name_end = header[..100].iter().position(|b| *b == 0).unwrap_or(100);
            names.push(String::from_utf8(header[..name_end].to_vec()).unwrap());
            let size_end = header[124..136].iter().position(|b| *b == 0).unwrap_or(12);
            let size = usize::from_str_radix(
                std::str::from_utf8(&header[124..124 + size_end])
                    .unwrap()
                    .trim(),
                8,
            )
            .unwrap();
            offset += 512 + size.div_ceil(512) * 512;
        }
        names
    }
}
