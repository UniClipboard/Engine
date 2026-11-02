//! 使用任务独占的日志文件验证公共诊断导出入口。
use std::error::Error;
use std::path::PathBuf;
use uc_application::facade::diagnostics::{DiagnosticsFacade, DiagnosticsFacadeDeps};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let logs_dir = PathBuf::from(arguments.next().ok_or("missing synthetic log directory")?);
    let destination = PathBuf::from(
        arguments
            .next()
            .ok_or("missing synthetic destination directory")?,
    );
    let facade = DiagnosticsFacade::new(DiagnosticsFacadeDeps {
        logs_dir,
        app_version: "0.0.0".into(),
        host_log_profile: None,
    });
    let outcome = facade.export_logs_to_dir(Some(24), destination).await?;
    std::io::Write::write_all(&mut std::io::stdout(), outcome.path.as_bytes())?;
    Ok(())
}
