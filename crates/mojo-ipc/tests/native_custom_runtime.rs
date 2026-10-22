//! 显式选择 Chromium 的真实运行库验收，使用独立进程隔离全局初始化。
use mojo_ipc::{Role, Runtime, RuntimeConfig};
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires trusted MOJO_CUSTOM_RUNTIME_LIBRARY and MOJO_CUSTOM_CHROMIUM_COMMIT"]
fn custom_runtime_pin_contract() {
    let library = PathBuf::from(
        std::env::var_os("MOJO_CUSTOM_RUNTIME_LIBRARY").expect("set custom native library"),
    );
    let commit = std::env::var("MOJO_CUSTOM_CHROMIUM_COMMIT").expect("set custom Chromium commit");
    assert_ne!(commit, RuntimeConfig::default().chromium_commit);
    if std::env::var_os("MOJO_LOAD_CONTRACT_CHILD").is_none() {
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "custom_runtime_pin_contract",
                "--ignored",
                "--nocapture",
            ])
            .env("MOJO_LOAD_CONTRACT_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    // SAFETY: 操作者显式选择了可信且与 SDK C ABI/布局一致的自定义原生库。
    let error = unsafe { Runtime::load(&library, Role::Broker) }.unwrap_err();
    assert!(error.to_string().contains("revision mismatch"));
    // SAFETY: 同一可信镜像；错误提交不得进入原生 runtime_init。
    let wrong = RuntimeConfig {
        chromium_commit: "0000000000000000000000000000000000000000",
    };
    assert!(unsafe { Runtime::load_with_config(&library, Role::Broker, wrong) }.is_err());
    let custom = RuntimeConfig {
        chromium_commit: &commit,
    };
    // SAFETY: 操作者审核的自定义提交、库和 C ABI 匹配。
    let runtime = unsafe { Runtime::load_with_config(&library, Role::Broker, custom) }.unwrap();
    assert!(unsafe { Runtime::load_with_config(&library, Role::Broker, custom) }.is_ok());
    // 已初始化后仍需核对配置，不能利用同路径和角色绕过默认官方 pin。
    assert!(unsafe { Runtime::load(&library, Role::Broker) }.is_err());
    assert!(unsafe { Runtime::load_with_config(&library, Role::Broker, wrong) }.is_err());
    assert!(unsafe { Runtime::load_with_config(&library, Role::Participant, custom) }.is_err());
    let _ = runtime.dispatch_runner();
}
