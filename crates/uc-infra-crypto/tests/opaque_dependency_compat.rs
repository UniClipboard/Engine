//! 通过私有子进程管道验证固定旧二进制与候选的实际密码消息及解密后资料兼容。
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};
use uc_core::crypto::domain::Passphrase;
use uc_core::membership::{AdmissionChannelPeerId, InvitationId, SpaceAdmissionId};
use uc_infra_crypto::space_admission_auth::{
    SpaceAdmissionAuth, SpaceAdmissionAuthContext, SpaceAdmissionKe1, SpaceAdmissionKe2,
    SpaceAdmissionKe3,
};

const PREFIX: &str = "UC_COMPAT_IPC:";

fn context() -> SpaceAdmissionAuthContext {
    SpaceAdmissionAuthContext::with_attempt_contract(
        SpaceAdmissionId::from_bytes([0x11; 32]).unwrap(),
        InvitationId::from_bytes([0x12; 32]).unwrap(),
        AdmissionChannelPeerId::from_bytes([0x13; 32]).unwrap(),
        AdmissionChannelPeerId::from_bytes([0x14; 32]).unwrap(),
        [0x15; 32],
    )
    .unwrap()
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::from_value(value.clone()).unwrap()
}

// 该入口只接受父测试私有管道，不将测试密码资料写入普通文件或日志。
#[test]
#[ignore = "仅由兼容测试作为私有管道子进程启动"]
fn compatibility_peer() {
    assert_eq!(std::env::var("UC_OPAQUE_COMPAT_WORKER").as_deref(), Ok("1"));
    let mut client = None;
    let mut server = None;
    let context = context();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let request: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let response = match request["op"].as_str().unwrap() {
            "register" => {
                let setup = SpaceAdmissionAuth::generate_server_setup();
                let record =
                    SpaceAdmissionAuth::register(&setup, &Passphrase::new("compat-test-password"))
                        .unwrap();
                json!({"setup": setup.encode_for_encryption().as_bytes(), "record": record.encode_for_encryption().as_bytes()})
            }
            "client_start" => {
                let password = request["password"].as_str().unwrap();
                let (state, ke1) =
                    SpaceAdmissionAuth::start_client(&Passphrase::new(password), &context).unwrap();
                client = Some(state);
                json!(ke1.encode_for_transport())
            }
            "server_start" => {
                let setup_bytes = bytes(&request["setup"]);
                let record_bytes = bytes(&request["record"]);
                let setup =
                    SpaceAdmissionAuth::decode_server_setup_after_decryption(&setup_bytes).unwrap();
                let record =
                    SpaceAdmissionAuth::decode_registration_after_decryption(&record_bytes)
                        .unwrap();
                assert_eq!(setup.encode_for_encryption().as_bytes(), setup_bytes);
                assert_eq!(record.encode_for_encryption().as_bytes(), record_bytes);
                let ke1 =
                    SpaceAdmissionKe1::decode_from_transport(&bytes(&request["ke1"])).unwrap();
                let (state, ke2) =
                    SpaceAdmissionAuth::start_server(&setup, &record, &context, ke1).unwrap();
                server = Some(state);
                json!(ke2.encode_for_transport())
            }
            "client_finish" => {
                let result = SpaceAdmissionKe2::decode_from_transport(&bytes(&request["ke2"]))
                    .and_then(|ke2| client.take().unwrap().finish(&context, ke2));
                match result {
                    Ok((key, ke3)) => {
                        json!({"ok": true, "key": key.into_core().unwrap().as_bytes(), "ke3": ke3.encode_for_transport()})
                    }
                    Err(_) => json!({"ok": false}),
                }
            }
            "server_finish" => {
                let result = SpaceAdmissionKe3::decode_from_transport(&bytes(&request["ke3"]))
                    .and_then(|ke3| server.take().unwrap().finish(&context, ke3));
                match result {
                    Ok(key) => json!({"ok": true, "key": key.into_core().unwrap().as_bytes()}),
                    Err(_) => json!({"ok": false}),
                }
            }
            _ => panic!("unknown test operation"),
        };
        writeln!(
            stdout,
            "{PREFIX}{}",
            serde_json::to_string(&response).unwrap()
        )
        .unwrap();
        stdout.flush().unwrap();
    }
}

struct Peer {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
}

impl Peer {
    fn spawn(binary: &Path) -> Self {
        let mut child = Command::new(binary)
            .env("UC_OPAQUE_COMPAT_WORKER", "1")
            .args(["--ignored", "--exact", "compatibility_peer", "--nocapture"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            input,
            output,
        }
    }

    fn call(&mut self, request: Value) -> Value {
        let input = self.input.as_mut().unwrap();
        serde_json::to_writer(&mut *input, &request).unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
        loop {
            let mut line = String::new();
            assert_ne!(
                self.output.read_line(&mut line).unwrap(),
                0,
                "compatibility peer terminated"
            );
            if let Some((_, value)) = line.split_once(PREFIX) {
                return serde_json::from_str(value).unwrap();
            }
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.input.take();
        if std::thread::panicking() {
            let _ = self.child.kill();
        }
        let status = self.child.wait();
        if !std::thread::panicking() {
            assert!(status.expect("compatibility peer status").success());
        }
    }
}

fn exchange(
    client: &mut Peer,
    server: &mut Peer,
    registration: &Value,
    password: &str,
    corrupt: bool,
) {
    let ke1 = client.call(json!({"op": "client_start", "password": password}));
    let mut ke2 = server.call(json!({"op": "server_start", "setup": registration["setup"], "record": registration["record"], "ke1": ke1}));
    if corrupt {
        let last = ke2.as_array_mut().unwrap().last_mut().unwrap();
        *last = json!(last.as_u64().unwrap() ^ 1);
    }
    let finished = client.call(json!({"op": "client_finish", "ke2": ke2}));
    if password != "compat-test-password" || corrupt {
        assert_eq!(finished["ok"], false);
        return;
    }
    assert_eq!(finished["ok"], true);
    let result = server.call(json!({"op": "server_finish", "ke3": finished["ke3"]}));
    assert_eq!(result["ok"], true);
    assert!(finished["key"] == result["key"], "continuation keys differ");
}

#[test]
#[ignore = "需要UC_OPAQUE_OLD_PEER指定固定旧二进制"]
fn fixed_old_and_candidate_are_compatible() {
    let old = std::env::var_os("UC_OPAQUE_OLD_PEER").expect("fixed old peer binary");
    let current = std::env::current_exe().unwrap();
    let mut old_peer = Peer::spawn(Path::new(&old));
    let mut candidate = Peer::spawn(&current);
    let old_registration = old_peer.call(json!({"op": "register"}));
    let new_registration = candidate.call(json!({"op": "register"}));
    // 双向混合角色均读取旧、新两种来源资料；每次真实握手使用独立状态。
    for registration in [&old_registration, &new_registration] {
        exchange(
            &mut old_peer,
            &mut candidate,
            registration,
            "compat-test-password",
            false,
        );
        exchange(
            &mut candidate,
            &mut old_peer,
            registration,
            "compat-test-password",
            false,
        );
        exchange(
            &mut candidate,
            &mut old_peer,
            registration,
            "wrong-password",
            false,
        );
        exchange(
            &mut old_peer,
            &mut candidate,
            registration,
            "wrong-password",
            false,
        );
        exchange(
            &mut candidate,
            &mut old_peer,
            registration,
            "compat-test-password",
            true,
        );
        exchange(
            &mut old_peer,
            &mut candidate,
            registration,
            "compat-test-password",
            true,
        );
    }
}
