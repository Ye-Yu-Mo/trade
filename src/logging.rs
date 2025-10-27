use anyhow::Result;
use flexi_logger::{Cleanup, Criterion, Duplicate, FileSpec, Logger, Naming};

const LOG_DIR: &str = "logs";
const APP_LOG_BASENAME: &str = "app";
const MAX_LOG_FILES: usize = 5;
const ROTATION_SIZE_BYTES: u64 = 10 * 1024 * 1024; // 10 MB per file

pub fn init_logging() -> Result<()> {
    std::fs::create_dir_all(LOG_DIR)?;

    Logger::try_with_str("info")?
        .log_to_file(
            FileSpec::default()
                .directory(LOG_DIR)
                .basename(APP_LOG_BASENAME),
        )
        .duplicate_to_stdout(Duplicate::All)
        .rotate(
            Criterion::Size(ROTATION_SIZE_BYTES),
            Naming::Numbers,
            Cleanup::KeepLogFiles(MAX_LOG_FILES),
        )
        .format_for_stdout(flexi_logger::detailed_format)
        .format_for_files(flexi_logger::detailed_format)
        .start()?;

    Ok(())
}

pub fn logs_directory() -> &'static str {
    LOG_DIR
}
