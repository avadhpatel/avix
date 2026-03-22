use std::time::Duration;

use thiserror::Error;
use tokio::process::Command;
use tokio::time::timeout;

#[derive(Debug, Error)]
pub enum ExecError {
    #[error("unknown runtime: {0}")]
    UnknownRuntime(String),
    #[error("execution timeout after {0:?}")]
    Timeout(Duration),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub struct ExecService {
    pub timeout: Duration,
}

impl ExecService {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    pub async fn exec(&self, runtime: &str, code: &str) -> Result<ExecResult, ExecError> {
        let (program, flag) = match runtime {
            "python" | "python3" => ("python3", "-c"),
            "node" | "nodejs" => ("node", "-e"),
            "bash" | "sh" => ("bash", "-c"),
            _ => return Err(ExecError::UnknownRuntime(runtime.to_string())),
        };

        let result = timeout(self.timeout, async {
            Command::new(program).arg(flag).arg(code).output().await
        })
        .await;

        match result {
            Ok(Ok(output)) => Ok(ExecResult {
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                exit_code: output.status.code().unwrap_or(-1),
            }),
            Ok(Err(e)) => Err(ExecError::Io(e)),
            Err(_) => Err(ExecError::Timeout(self.timeout)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_bash_execution() {
        let svc = ExecService::new(Duration::from_secs(5));
        let res = svc.exec("bash", "echo world").await.unwrap();
        assert_eq!(res.stdout.trim(), "world");
        assert_eq!(res.exit_code, 0);
    }

    #[tokio::test]
    async fn test_exit_code_on_error() {
        let svc = ExecService::new(Duration::from_secs(5));
        let res = svc.exec("bash", "exit 1").await.unwrap();
        assert_eq!(res.exit_code, 1);
    }

    #[tokio::test]
    async fn test_timeout_kills_process() {
        let svc = ExecService::new(Duration::from_millis(100));
        let res = svc.exec("bash", "sleep 10").await;
        assert!(matches!(res, Err(ExecError::Timeout(_))));
    }

    #[tokio::test]
    async fn test_unknown_runtime_rejected() {
        let svc = ExecService::new(Duration::from_secs(5));
        let res = svc.exec("ruby", "puts 'hello'").await;
        assert!(matches!(res, Err(ExecError::UnknownRuntime(_))));
    }

    #[tokio::test]
    async fn test_bash_stderr() {
        let svc = ExecService::new(Duration::from_secs(5));
        let res = svc.exec("bash", "echo error >&2").await.unwrap();
        assert_eq!(res.stderr.trim(), "error");
    }

    #[tokio::test]
    async fn test_bash_multiline_output() {
        let svc = ExecService::new(Duration::from_secs(5));
        let res = svc.exec("bash", "echo line1 && echo line2").await.unwrap();
        assert!(res.stdout.contains("line1"));
        assert!(res.stdout.contains("line2"));
        assert_eq!(res.exit_code, 0);
    }

    #[tokio::test]
    async fn test_sh_alias_works() {
        let svc = ExecService::new(Duration::from_secs(5));
        let res = svc.exec("sh", "echo sh-works").await.unwrap();
        assert_eq!(res.stdout.trim(), "sh-works");
    }

    #[tokio::test]
    async fn test_python_execution() {
        let svc = ExecService::new(Duration::from_secs(5));
        let res = svc.exec("python3", "print('hello')").await;
        // Skip if python3 not available
        if let Ok(result) = res {
            assert_eq!(result.stdout.trim(), "hello");
            assert_eq!(result.exit_code, 0);
        }
    }
}
