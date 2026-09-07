use super::*;
use wiremock::matchers::body_partial_json;

// 严格按短码查找，不能用任意已登记邀请兜底掩盖前导零或分隔符丢失。
async fn directory() -> MockServer {
    let server = MockServer::start().await;
    let ticket = Arc::new(Mutex::new(None::<String>));
    let created_ticket = Arc::clone(&ticket);
    Mock::given(method("POST"))
        .and(path("/v1/pairings"))
        .and(body_partial_json(serde_json::json!({"codeLength": 6})))
        .respond_with(move |request: &Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            *created_ticket.lock().unwrap() =
                Some(body["sponsorTicket"].as_str().unwrap().to_owned());
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": "000-001", "expiresAtMs": EXPIRES_AT_MS,
            }))
        })
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/pairings/resolve"))
        .and(body_partial_json(serde_json::json!({"code": "000-001"})))
        .respond_with(move |_: &Request| {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sponsorTicket": ticket.lock().unwrap().as_ref().unwrap(),
                "sponsorEndpointId": "local-e2e", "expiresAtMs": EXPIRES_AT_MS,
            }))
        })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/pairings/consume"))
        .and(body_partial_json(serde_json::json!({"code": "000-001"})))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    server
}

async fn join_using_short_code(base_url: String, switching: bool, expected_code: Option<&str>) {
    let sponsor_harness = DeviceHarness::new(base_url.clone());
    let joiner_harness = DeviceHarness::new(base_url);
    let sponsor = sponsor_harness.start().await;
    let joiner = joiner_harness.start().await;
    let space_id = create_space(&sponsor, "Six Digit Sponsor").await.0;
    if switching {
        let previous = create_space(&joiner, "Existing Device").await.0;
        assert_ne!(previous, space_id);
    }
    let OperationResult::InvitationIssued {
        invitation_code: code,
        ..
    } = sponsor
        .execute(Operation::IssueInvitation)
        .await
        .expect("issue short invitation")
    else {
        panic!("unexpected invitation result");
    };
    let (left, right) = code.split_once('-').expect("short code separator");
    assert_eq!((left.len(), right.len()), (3, 3));
    assert!(left
        .bytes()
        .chain(right.bytes())
        .all(|b| b.is_ascii_digit()));
    if let Some(expected) = expected_code {
        assert_eq!(code, expected);
    }
    join_with_invitation(&joiner, "Six Digit Joiner", &space_id, code).await;
    for engine in [&sponsor, &joiner] {
        wait_for_active_member_count(engine, 2).await;
        engine.shutdown(SHUTDOWN_TIMEOUT).await.expect("shutdown");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn leading_zero_code_joins_fresh_device() {
    let server = directory().await;
    join_using_short_code(server.uri(), false, Some("000-001")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn leading_zero_code_switches_existing_space() {
    let server = directory().await;
    join_using_short_code(server.uri(), true, Some("000-001")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn unavailable_directory_joins_using_local_six_digit_code() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    join_using_short_code(server.uri(), false, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "需要启动本地 rendezvous，并设置 UC_SIX_DIGIT_RENDEZVOUS_URL"]
async fn local_rendezvous_joins_and_switches_using_six_digit_codes() {
    let base_url = std::env::var("UC_SIX_DIGIT_RENDEZVOUS_URL").expect("local rendezvous URL");
    assert!(base_url.starts_with("http://127.0.0.1:"));
    join_using_short_code(base_url.clone(), false, None).await;
    join_using_short_code(base_url, true, None).await;
}
