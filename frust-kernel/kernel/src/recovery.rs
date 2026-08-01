//! First-class, topology-aware backup and restore operations.
//!
//! A backup is published as one directory containing `dump.surql` and a
//! checksummed manifest. Restore is deliberately narrower than backup:
//! shared-database topologies are refused, the target scope must exactly match
//! the manifest, and a safety backup must complete before the live database is
//! reset. SurrealDB import is not transactional with the reset, so any failure
//! after that point names the safety backup in its error.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::tenancy::{ResolvedTenant, Tenancy};

const FORMAT_VERSION: u32 = 1;
const DUMP_FILE: &str = "dump.surql";
const MANIFEST_FILE: &str = "manifest.json";

pub const USAGE: &str = concat!(
    "usage:\n",
    "  frust backup --tenant <slug> --output <new-directory> ",
    "[--surreal-bin <path>] [--dry-run]\n",
    "  frust backup verify --input <backup-directory>\n",
    "  frust restore --tenant <slug> --input <backup-directory> ",
    "--safety-backup <new-directory> --confirm-drop <namespace/database> ",
    "[--surreal-bin <path>] [--dry-run]",
);

#[derive(Debug)]
pub struct RecoveryError(String);

impl RecoveryError {
    fn new(detail: impl Into<String>) -> Self {
        Self(detail.into())
    }
}

impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RecoveryError {}

impl From<io::Error> for RecoveryError {
    fn from(value: io::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<serde_json::Error> for RecoveryError {
    fn from(value: serde_json::Error) -> Self {
        Self::new(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryCommand {
    Backup {
        tenant: String,
        output: PathBuf,
        surreal_bin: PathBuf,
        dry_run: bool,
    },
    Verify {
        input: PathBuf,
    },
    Restore {
        tenant: String,
        input: PathBuf,
        safety_backup: PathBuf,
        confirm_drop: Option<String>,
        surreal_bin: PathBuf,
        dry_run: bool,
    },
}

impl RecoveryCommand {
    /// The tenant the command deliberately registers when no environment
    /// roster was supplied. Offline verification needs no database target.
    pub fn tenant(&self) -> Option<&str> {
        match self {
            Self::Backup { tenant, .. } | Self::Restore { tenant, .. } => Some(tenant),
            Self::Verify { .. } => None,
        }
    }
}

fn option(args: &[String], name: &str) -> Result<Option<String>, RecoveryError> {
    let mut found = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == name {
            if found.is_some() {
                return Err(RecoveryError::new(format!(
                    "{name} was supplied more than once"
                )));
            }
            let value = args
                .get(i + 1)
                .filter(|value| !value.starts_with("--"))
                .ok_or_else(|| RecoveryError::new(format!("{name} requires a value")))?;
            found = Some(value.clone());
            i += 2;
        } else {
            i += 1;
        }
    }
    Ok(found)
}

fn required(args: &[String], name: &str) -> Result<String, RecoveryError> {
    option(args, name)?.ok_or_else(|| RecoveryError::new(format!("missing {name}\n{USAGE}")))
}

fn validate_flags(
    args: &[String],
    valued: &[&str],
    switches: &[&str],
) -> Result<(), RecoveryError> {
    let mut i = 0;
    while i < args.len() {
        let flag = &args[i];
        if valued.contains(&flag.as_str()) {
            if args.get(i + 1).is_none_or(|value| value.starts_with("--")) {
                return Err(RecoveryError::new(format!("{flag} requires a value")));
            }
            i += 2;
        } else if switches.contains(&flag.as_str()) {
            i += 1;
        } else {
            return Err(RecoveryError::new(format!(
                "unknown recovery argument {flag:?}\n{USAGE}"
            )));
        }
    }
    Ok(())
}

fn surreal_bin(args: &[String]) -> Result<PathBuf, RecoveryError> {
    Ok(option(args, "--surreal-bin")?
        .or_else(|| std::env::var("FRUST_SURREAL_BIN").ok())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("surreal")))
}

/// Parse only recovery commands. `None` leaves the existing serve/one-shot
/// boot path untouched.
pub fn parse(args: &[String]) -> Result<Option<RecoveryCommand>, RecoveryError> {
    match args.first().map(String::as_str) {
        Some("backup") if args.get(1).map(String::as_str) == Some("verify") => {
            let tail = &args[2..];
            validate_flags(tail, &["--input"], &[])?;
            Ok(Some(RecoveryCommand::Verify {
                input: required(tail, "--input")?.into(),
            }))
        }
        Some("backup") => {
            let tail = &args[1..];
            validate_flags(
                tail,
                &["--tenant", "--output", "--surreal-bin"],
                &["--dry-run"],
            )?;
            Ok(Some(RecoveryCommand::Backup {
                tenant: required(tail, "--tenant")?,
                output: required(tail, "--output")?.into(),
                surreal_bin: surreal_bin(tail)?,
                dry_run: tail.iter().any(|arg| arg == "--dry-run"),
            }))
        }
        Some("restore") => {
            let tail = &args[1..];
            validate_flags(
                tail,
                &[
                    "--tenant",
                    "--input",
                    "--safety-backup",
                    "--confirm-drop",
                    "--surreal-bin",
                ],
                &["--dry-run"],
            )?;
            Ok(Some(RecoveryCommand::Restore {
                tenant: required(tail, "--tenant")?,
                input: required(tail, "--input")?.into(),
                safety_backup: required(tail, "--safety-backup")?.into(),
                confirm_drop: option(tail, "--confirm-drop")?,
                surreal_bin: surreal_bin(tail)?,
                dry_run: tail.iter().any(|arg| arg == "--dry-run"),
            }))
        }
        _ => Ok(None),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupManifest {
    pub format_version: u32,
    pub created_unix_seconds: u64,
    pub tenant: String,
    pub topology: String,
    pub namespace: String,
    pub database: String,
    pub tenant_isolated: bool,
    pub dump_file: String,
    pub dump_bytes: u64,
    pub dump_sha256: String,
}

impl BackupManifest {
    pub fn scope(&self) -> String {
        format!("{}/{}", self.namespace, self.database)
    }
}

#[derive(Debug)]
struct TargetPlan {
    tenant: String,
    topology: String,
    namespace: String,
    database: String,
    tenant_isolated: bool,
}

impl TargetPlan {
    fn from_target(target: &ResolvedTenant) -> Result<Self, RecoveryError> {
        let plan = target.strategy().backup_plan(target);
        let tenant_isolated = plan.is_tenant_isolated();
        let database = plan.database.ok_or_else(|| {
            RecoveryError::new("SurrealDB export/import requires a database-scoped backup plan")
        })?;
        Ok(Self {
            tenant: target.tenant_id().as_str().to_string(),
            topology: target.strategy().name().to_string(),
            namespace: plan.namespace.as_str().to_string(),
            database: database.as_str().to_string(),
            tenant_isolated,
        })
    }

    fn scope(&self) -> String {
        format!("{}/{}", self.namespace, self.database)
    }

    fn matches(&self, manifest: &BackupManifest) -> bool {
        self.tenant == manifest.tenant
            && self.topology == manifest.topology
            && self.namespace == manifest.namespace
            && self.database == manifest.database
            && self.tenant_isolated == manifest.tenant_isolated
    }
}

fn hash_file(path: &Path) -> Result<(u64, String), RecoveryError> {
    let mut file = File::open(path)
        .map_err(|e| RecoveryError::new(format!("open {}: {e}", path.display())))?;
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    let mut buf = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        digest.update(&buf[..read]);
    }
    Ok((bytes, format!("{:x}", digest.finalize())))
}

/// Verify the backup container without contacting a database.
pub fn verify_backup(input: &Path) -> Result<BackupManifest, RecoveryError> {
    if !input.is_dir() {
        return Err(RecoveryError::new(format!(
            "backup path is not a directory: {}",
            input.display()
        )));
    }
    let manifest_path = input.join(MANIFEST_FILE);
    let manifest: BackupManifest = serde_json::from_reader(
        File::open(&manifest_path)
            .map_err(|e| RecoveryError::new(format!("open {}: {e}", manifest_path.display())))?,
    )
    .map_err(|e| RecoveryError::new(format!("parse {}: {e}", manifest_path.display())))?;
    if manifest.format_version != FORMAT_VERSION {
        return Err(RecoveryError::new(format!(
            "unsupported backup format {} (this binary supports {})",
            manifest.format_version, FORMAT_VERSION
        )));
    }
    if manifest.dump_file != DUMP_FILE {
        return Err(RecoveryError::new(format!(
            "manifest dump_file must be {DUMP_FILE:?}, got {:?}",
            manifest.dump_file
        )));
    }
    let dump = input.join(DUMP_FILE);
    let (bytes, sha256) = hash_file(&dump)?;
    if bytes == 0 {
        return Err(RecoveryError::new("backup dump is empty"));
    }
    if bytes != manifest.dump_bytes || sha256 != manifest.dump_sha256 {
        return Err(RecoveryError::new(format!(
            "backup integrity mismatch: manifest bytes={} sha256={}, actual bytes={} sha256={}",
            manifest.dump_bytes, manifest.dump_sha256, bytes, sha256
        )));
    }
    Ok(manifest)
}

trait RecoveryOps {
    fn export(&self, target: &ResolvedTenant, dump: &Path) -> Result<(), RecoveryError>;
    fn reset_database(&self, target: &ResolvedTenant) -> Result<(), RecoveryError>;
    fn import(&self, target: &ResolvedTenant, dump: &Path) -> Result<(), RecoveryError>;
    fn rotate_access(&self, target: &ResolvedTenant) -> Result<(), RecoveryError>;
    fn assert_keyguard(&self, target: &ResolvedTenant) -> Result<(), RecoveryError>;
}

struct SystemOps {
    surreal_bin: PathBuf,
}

impl SystemOps {
    fn surreal(
        &self,
        verb: &str,
        target: &ResolvedTenant,
        file: &Path,
    ) -> Result<(), RecoveryError> {
        let plan = TargetPlan::from_target(target)?;
        let mut args: Vec<OsString> = vec![
            verb.into(),
            "--endpoint".into(),
            target.endpoint().into(),
            "--namespace".into(),
            plan.namespace.into(),
            "--database".into(),
            plan.database.into(),
            "--log".into(),
            "error".into(),
            file.as_os_str().to_owned(),
        ];
        let conn = target.conn();
        let output = ProcessCommand::new(&self.surreal_bin)
            .args(args.drain(..))
            // Credentials do not enter argv/process listings.
            .env("SURREAL_USER", &conn.root_user)
            .env("SURREAL_PASS", &conn.root_pass)
            .env("SURREAL_AUTH_LEVEL", "root")
            .env_remove("SURREAL_TOKEN")
            .output()
            .map_err(|e| {
                RecoveryError::new(format!(
                    "could not start {} {verb}: {e}",
                    self.surreal_bin.display()
                ))
            })?;
        if output.status.success() {
            return Ok(());
        }
        let mut detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if detail.is_empty() {
            detail = String::from_utf8_lossy(&output.stdout).trim().to_string();
        }
        if !conn.root_pass.is_empty() {
            detail = detail.replace(&conn.root_pass, "<redacted>");
        }
        if detail.len() > 4096 {
            detail.truncate(4096);
            detail.push_str("...<truncated>");
        }
        Err(RecoveryError::new(format!(
            "surreal {verb} failed with {}: {detail}",
            output.status
        )))
    }
}

impl RecoveryOps for SystemOps {
    fn export(&self, target: &ResolvedTenant, dump: &Path) -> Result<(), RecoveryError> {
        self.surreal("export", target, dump)
    }

    fn reset_database(&self, target: &ResolvedTenant) -> Result<(), RecoveryError> {
        crate::provision::reset_database_for_restore(target)
            .map_err(|e| RecoveryError::new(format!("reset restore target: {e}")))
    }

    fn import(&self, target: &ResolvedTenant, dump: &Path) -> Result<(), RecoveryError> {
        self.surreal("import", target, dump)
    }

    fn rotate_access(&self, target: &ResolvedTenant) -> Result<(), RecoveryError> {
        crate::provision::rotate_restored_access(target)
            .map_err(|e| RecoveryError::new(format!("rotate restored access: {e}")))
    }

    fn assert_keyguard(&self, target: &ResolvedTenant) -> Result<(), RecoveryError> {
        crate::keyguard::assert_access_key_is_not_redacted(&crate::db::scoped_db(target))
            .map_err(|e| RecoveryError::new(format!("post-restore keyguard: {e}")))
    }
}

fn staging_path(output: &Path) -> Result<PathBuf, RecoveryError> {
    let file_name = output.file_name().ok_or_else(|| {
        RecoveryError::new(format!(
            "backup output needs a directory name: {}",
            output.display()
        ))
    })?;
    let mut staging_name = file_name.to_os_string();
    staging_name.push(format!(".partial-{}", std::process::id()));
    Ok(output.with_file_name(staging_name))
}

fn create_backup(
    ops: &dyn RecoveryOps,
    target: &ResolvedTenant,
    output: &Path,
) -> Result<BackupManifest, RecoveryError> {
    if output.exists() {
        return Err(RecoveryError::new(format!(
            "backup output already exists; refusing to overwrite: {}",
            output.display()
        )));
    }
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|e| RecoveryError::new(format!("create {}: {e}", parent.display())))?;
    let staging = staging_path(output)?;
    if staging.exists() {
        return Err(RecoveryError::new(format!(
            "staging path already exists; inspect it instead of overwriting it: {}",
            staging.display()
        )));
    }
    fs::create_dir(&staging)?;

    let result = (|| {
        let target_plan = TargetPlan::from_target(target)?;
        let dump = staging.join(DUMP_FILE);
        ops.export(target, &dump)?;
        let (dump_bytes, dump_sha256) = hash_file(&dump)?;
        if dump_bytes == 0 {
            return Err(RecoveryError::new("surreal export produced an empty dump"));
        }
        let created_unix_seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| RecoveryError::new(format!("system clock precedes Unix epoch: {e}")))?
            .as_secs();
        let manifest = BackupManifest {
            format_version: FORMAT_VERSION,
            created_unix_seconds,
            tenant: target_plan.tenant,
            topology: target_plan.topology,
            namespace: target_plan.namespace,
            database: target_plan.database,
            tenant_isolated: target_plan.tenant_isolated,
            dump_file: DUMP_FILE.into(),
            dump_bytes,
            dump_sha256,
        };
        let mut manifest_file = File::create(staging.join(MANIFEST_FILE))?;
        serde_json::to_writer_pretty(&mut manifest_file, &manifest)?;
        manifest_file.write_all(b"\n")?;
        manifest_file.sync_all()?;
        drop(manifest_file);
        OpenOptions::new().write(true).open(&dump)?.sync_all()?;
        let verified = verify_backup(&staging)?;
        fs::rename(&staging, output).map_err(|e| {
            RecoveryError::new(format!(
                "publish backup {} -> {}: {e}",
                staging.display(),
                output.display()
            ))
        })?;
        Ok(verified)
    })();

    if result.is_err() && staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn verify_matches_target(
    input: &Path,
    target_plan: &TargetPlan,
) -> Result<BackupManifest, RecoveryError> {
    let manifest = verify_backup(input)?;
    if !target_plan.matches(&manifest) {
        return Err(RecoveryError::new(format!(
            "backup scope mismatch: backup tenant={} topology={} target={}; configured tenant={} \
             topology={} target={}",
            manifest.tenant,
            manifest.topology,
            manifest.scope(),
            target_plan.tenant,
            target_plan.topology,
            target_plan.scope(),
        )));
    }
    Ok(manifest)
}

fn restore(
    ops: &dyn RecoveryOps,
    target: &ResolvedTenant,
    input: &Path,
    safety_backup: &Path,
    confirm_drop: Option<&str>,
) -> Result<BackupManifest, RecoveryError> {
    let target_plan = TargetPlan::from_target(target)?;
    if !target_plan.tenant_isolated {
        return Err(RecoveryError::new(format!(
            "refusing tenant restore for topology {}: target {} contains shared tenant data; \
             use a separately designed whole-database recovery procedure",
            target_plan.topology,
            target_plan.scope()
        )));
    }
    let manifest = verify_matches_target(input, &target_plan)?;
    if confirm_drop != Some(target_plan.scope().as_str()) {
        return Err(RecoveryError::new(format!(
            "restore drops and recreates {}; repeat with --confirm-drop {}",
            target_plan.scope(),
            target_plan.scope()
        )));
    }
    if input == safety_backup || safety_backup.starts_with(input) {
        return Err(RecoveryError::new(
            "--safety-backup must be outside the backup being restored",
        ));
    }

    create_backup(ops, target, safety_backup).map_err(|e| {
        RecoveryError::new(format!(
            "safety backup failed; live target was not changed: {e}"
        ))
    })?;
    // The safety export may take minutes on a real volume. Re-hash the source
    // after it completes so a changed/truncated input cannot be followed by a
    // destructive reset on the strength of a stale preflight result.
    let pre_drop = verify_matches_target(input, &target_plan)?;
    if pre_drop != manifest {
        return Err(RecoveryError::new(
            "restore input changed during preflight; live target was not changed",
        ));
    }
    ops.reset_database(target).map_err(|e| {
        RecoveryError::new(format!(
            "restore target reset failed; safety backup is at {}: {e}",
            safety_backup.display()
        ))
    })?;
    ops.import(target, &input.join(DUMP_FILE)).map_err(|e| {
        RecoveryError::new(format!(
            "restore import failed after target reset; DO NOT SERVE this target; safety backup is \
             at {}: {e}",
            safety_backup.display()
        ))
    })?;
    ops.rotate_access(target).map_err(|e| {
        RecoveryError::new(format!(
            "restore imported but signing-key remediation failed; DO NOT SERVE this target; \
             safety backup is at {}: {e}",
            safety_backup.display()
        ))
    })?;
    ops.assert_keyguard(target).map_err(|e| {
        RecoveryError::new(format!(
            "restore imported but failed the signing-key guard; DO NOT SERVE this target; \
             safety backup is at {}: {e}",
            safety_backup.display()
        ))
    })?;
    Ok(manifest)
}

/// Execute a parsed command. Verification is offline; all other operations
/// resolve their tenant through the configured registry before doing I/O.
pub fn run(command: RecoveryCommand, tenancy: Option<&Tenancy>) -> Result<String, RecoveryError> {
    match command {
        RecoveryCommand::Verify { input } => {
            let manifest = verify_backup(&input)?;
            Ok(format!(
                "backup verified: tenant={} topology={} target={} bytes={} sha256={}",
                manifest.tenant,
                manifest.topology,
                manifest.scope(),
                manifest.dump_bytes,
                manifest.dump_sha256
            ))
        }
        RecoveryCommand::Backup {
            tenant,
            output,
            surreal_bin,
            dry_run,
        } => {
            let tenancy =
                tenancy.ok_or_else(|| RecoveryError::new("backup needs tenancy config"))?;
            let target = tenancy
                .resolve(&tenant)
                .map_err(|e| RecoveryError::new(e.to_string()))?;
            let plan = TargetPlan::from_target(&target)?;
            if dry_run {
                return Ok(format!(
                    "dry run: backup tenant={} topology={} target={} isolated={} output={}",
                    plan.tenant,
                    plan.topology,
                    plan.scope(),
                    plan.tenant_isolated,
                    output.display()
                ));
            }
            let manifest = create_backup(&SystemOps { surreal_bin }, &target, &output)?;
            Ok(format!(
                "backup complete: tenant={} target={} output={} bytes={} sha256={}",
                manifest.tenant,
                manifest.scope(),
                output.display(),
                manifest.dump_bytes,
                manifest.dump_sha256
            ))
        }
        RecoveryCommand::Restore {
            tenant,
            input,
            safety_backup,
            confirm_drop,
            surreal_bin,
            dry_run,
        } => {
            let tenancy =
                tenancy.ok_or_else(|| RecoveryError::new("restore needs tenancy config"))?;
            let target = tenancy
                .resolve(&tenant)
                .map_err(|e| RecoveryError::new(e.to_string()))?;
            let plan = TargetPlan::from_target(&target)?;
            if !plan.tenant_isolated {
                return Err(RecoveryError::new(format!(
                    "refusing tenant restore for topology {}: target {} contains shared tenant data",
                    plan.topology,
                    plan.scope()
                )));
            }
            verify_matches_target(&input, &plan)?;
            if safety_backup.exists() {
                return Err(RecoveryError::new(format!(
                    "safety backup output already exists; refusing to overwrite: {}",
                    safety_backup.display()
                )));
            }
            if dry_run {
                return Ok(format!(
                    "dry run: restore tenant={} topology={} target={} input={} safety_backup={}; \
                     steps=verify,safety-export,drop-and-create,import,rotate-access,keyguard",
                    plan.tenant,
                    plan.topology,
                    plan.scope(),
                    input.display(),
                    safety_backup.display()
                ));
            }
            let manifest = restore(
                &SystemOps { surreal_bin },
                &target,
                &input,
                &safety_backup,
                confirm_drop.as_deref(),
            )?;
            Ok(format!(
                "restore complete: tenant={} target={} source={} safety_backup={}; all sessions \
                 invalidated and keyguard passed",
                manifest.tenant,
                manifest.scope(),
                input.display(),
                safety_backup.display()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use crate::tenancy::{test_tenancy, DEFAULT_TENANT};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frust-recovery-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    fn tenancy(strategy: &str, database: Option<&str>) -> Tenancy {
        test_tenancy(strategy, database).expect("tenancy")
    }

    fn isolated_strategy() -> String {
        ["database", "per", "tenant"].join("-")
    }

    #[derive(Default)]
    struct FakeOps {
        calls: Mutex<Vec<&'static str>>,
        fail_at: Option<&'static str>,
    }

    impl FakeOps {
        fn call(&self, name: &'static str) -> Result<(), RecoveryError> {
            self.calls.lock().expect("calls").push(name);
            if self.fail_at == Some(name) {
                Err(RecoveryError::new(format!("injected {name} failure")))
            } else {
                Ok(())
            }
        }
    }

    impl RecoveryOps for FakeOps {
        fn export(&self, _target: &ResolvedTenant, dump: &Path) -> Result<(), RecoveryError> {
            self.call("export")?;
            fs::write(dump, b"-- isolated fake SurrealDB export\nRETURN 1;\n")?;
            Ok(())
        }

        fn reset_database(&self, _target: &ResolvedTenant) -> Result<(), RecoveryError> {
            self.call("reset")
        }

        fn import(&self, _target: &ResolvedTenant, _dump: &Path) -> Result<(), RecoveryError> {
            self.call("import")
        }

        fn rotate_access(&self, _target: &ResolvedTenant) -> Result<(), RecoveryError> {
            self.call("rotate")
        }

        fn assert_keyguard(&self, _target: &ResolvedTenant) -> Result<(), RecoveryError> {
            self.call("keyguard")
        }
    }

    #[test]
    fn parser_rejects_unknown_and_duplicate_arguments() {
        let bad = vec!["backup".into(), "--wat".into()];
        assert!(parse(&bad).unwrap_err().to_string().contains("unknown"));

        let duplicate = vec![
            "backup".into(),
            "--tenant".into(),
            "one".into(),
            "--tenant".into(),
            "two".into(),
            "--output".into(),
            "x".into(),
        ];
        assert!(parse(&duplicate)
            .unwrap_err()
            .to_string()
            .contains("more than once"));
    }

    #[test]
    fn backup_is_published_only_after_export_and_manifest_verify() {
        let root = temp_dir();
        let output = root.join("backup");
        let strategy = isolated_strategy();
        let t = tenancy(&strategy, None);
        let target = t.resolve(DEFAULT_TENANT).expect("target");
        let manifest = create_backup(&FakeOps::default(), &target, &output).expect("backup");

        assert!(output.join(DUMP_FILE).is_file());
        assert_eq!(verify_backup(&output).expect("verify"), manifest);
        assert!(!staging_path(&output).expect("staging").exists());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn failed_export_leaves_no_apparently_complete_backup() {
        let root = temp_dir();
        let output = root.join("backup");
        let strategy = isolated_strategy();
        let t = tenancy(&strategy, None);
        let target = t.resolve(DEFAULT_TENANT).expect("target");
        let ops = FakeOps {
            fail_at: Some("export"),
            ..Default::default()
        };

        assert!(create_backup(&ops, &target, &output).is_err());
        assert!(!output.exists());
        assert!(!staging_path(&output).expect("staging").exists());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn corruption_is_detected_before_restore_work() {
        let root = temp_dir();
        let input = root.join("input");
        let safety = root.join("safety");
        let strategy = isolated_strategy();
        let t = tenancy(&strategy, None);
        let target = t.resolve(DEFAULT_TENANT).expect("target");
        create_backup(&FakeOps::default(), &target, &input).expect("input backup");
        fs::write(input.join(DUMP_FILE), b"tampered").expect("tamper");
        let ops = FakeOps::default();

        let err = restore(
            &ops,
            &target,
            &input,
            &safety,
            Some(&TargetPlan::from_target(&target).unwrap().scope()),
        )
        .unwrap_err();
        assert!(err.to_string().contains("integrity mismatch"));
        assert!(ops.calls.lock().unwrap().is_empty());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn restore_requires_exact_confirmation_and_a_safety_backup_first() {
        let root = temp_dir();
        let input = root.join("input");
        let safety = root.join("safety");
        let strategy = isolated_strategy();
        let t = tenancy(&strategy, None);
        let target = t.resolve(DEFAULT_TENANT).expect("target");
        create_backup(&FakeOps::default(), &target, &input).expect("input backup");

        let no_confirm = FakeOps::default();
        assert!(restore(&no_confirm, &target, &input, &safety, None)
            .unwrap_err()
            .to_string()
            .contains("--confirm-drop"));
        assert!(no_confirm.calls.lock().unwrap().is_empty());

        let ops = FakeOps::default();
        let scope = TargetPlan::from_target(&target).unwrap().scope();
        restore(&ops, &target, &input, &safety, Some(&scope)).expect("restore");
        assert_eq!(
            *ops.calls.lock().unwrap(),
            vec!["export", "reset", "import", "rotate", "keyguard"]
        );
        assert!(verify_backup(&safety).is_ok());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn import_failure_names_the_completed_safety_backup() {
        let root = temp_dir();
        let input = root.join("input");
        let safety = root.join("safety");
        let strategy = isolated_strategy();
        let t = tenancy(&strategy, None);
        let target = t.resolve(DEFAULT_TENANT).expect("target");
        create_backup(&FakeOps::default(), &target, &input).expect("input backup");
        let ops = FakeOps {
            fail_at: Some("import"),
            ..Default::default()
        };
        let scope = TargetPlan::from_target(&target).unwrap().scope();

        let err = restore(&ops, &target, &input, &safety, Some(&scope)).unwrap_err();
        assert!(err.to_string().contains("DO NOT SERVE"));
        assert!(err.to_string().contains(&safety.display().to_string()));
        assert!(verify_backup(&safety).is_ok());
        assert_eq!(
            *ops.calls.lock().unwrap(),
            vec!["export", "reset", "import"]
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn shared_database_topology_refuses_tenant_restore_before_io() {
        let root = temp_dir();
        let input = root.join("input");
        let safety = root.join("safety");
        let t = tenancy("single", Some("shared"));
        let target = t.resolve(DEFAULT_TENANT).expect("target");
        let ops = FakeOps::default();

        let scope = TargetPlan::from_target(&target).expect("plan").scope();
        let err = restore(&ops, &target, &input, &safety, Some(&scope)).unwrap_err();
        assert!(err.to_string().contains("shared tenant data"));
        assert!(ops.calls.lock().unwrap().is_empty());
        fs::remove_dir_all(root).expect("cleanup");
    }
}
