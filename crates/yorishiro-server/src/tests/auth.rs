use axum::http::StatusCode;
use sqlx::PgPool;
use uuid::Uuid;
use yorishiro_core::repositories::tenancy;

use super::helpers::*;
use crate::*;

#[sqlx::test(migrations = "../../migrations")]
async fn signup_consumes_invite_and_creates_membership(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let workspace = tenancy::create_workspace(&pool, tenant.id, "main", None)
        .await
        .unwrap();
    let (_invite, token) = tenancy::create_invite(
        &pool,
        tenant.id,
        "new@example.com",
        tenancy::MembershipRole::Member,
        chrono::Duration::hours(1),
    )
    .await
    .unwrap();

    let app = build_app(test_state(pool.clone()), None);

    let response = rest_request(
        &app,
        "POST",
        "/auth/signup",
        None,
        Some(serde_json::json!({
            "invite_token": token,
            "password": "hunter2-hunter2",
            "display_name": "New Member",
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = rest_json_body(response).await;
    assert_eq!(body["email"], "new@example.com");
    assert_eq!(body["tenant_id"], tenant.id.to_string());
    assert_eq!(body["role"], "member");
    assert_eq!(body["workspaces"][0]["id"], workspace.id.to_string());

    let role = tenancy::get_membership_role(
        &pool,
        tenant.id,
        Uuid::parse_str(body["user_id"].as_str().unwrap()).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(role, Some(tenancy::MembershipRole::Member));
}

#[sqlx::test(migrations = "../../migrations")]
async fn signup_rejects_an_already_used_invite_token(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    tenancy::create_workspace(&pool, tenant.id, "main", None)
        .await
        .unwrap();
    let (_invite, token) = tenancy::create_invite(
        &pool,
        tenant.id,
        "reuse@example.com",
        tenancy::MembershipRole::Member,
        chrono::Duration::hours(1),
    )
    .await
    .unwrap();

    let app = build_app(test_state(pool), None);
    let signup_body = Some(serde_json::json!({
        "invite_token": token,
        "password": "hunter2-hunter2",
    }));

    let response = rest_request(&app, "POST", "/auth/signup", None, signup_body.clone()).await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = rest_request(&app, "POST", "/auth/signup", None, signup_body).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_issues_an_api_key_scoped_to_the_members_role(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let workspace = tenancy::create_workspace(&pool, tenant.id, "main", None)
        .await
        .unwrap();
    let (_invite, token) = tenancy::create_invite(
        &pool,
        tenant.id,
        "member@example.com",
        tenancy::MembershipRole::Member,
        chrono::Duration::hours(1),
    )
    .await
    .unwrap();

    let app = build_app(test_state(pool), None);

    let response = rest_request(
        &app,
        "POST",
        "/auth/signup",
        None,
        Some(serde_json::json!({
            "invite_token": token,
            "password": "hunter2-hunter2",
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = rest_request(
        &app,
        "POST",
        "/auth/login",
        None,
        Some(serde_json::json!({
            "email": "member@example.com",
            "password": "hunter2-hunter2",
            "workspace_id": workspace.id,
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = rest_json_body(response).await;
    assert_eq!(body["scope"], "write");
    let api_key = body["api_key"].as_str().unwrap();

    let response = rest_request(
        &app,
        "GET",
        "/api/entities",
        Some(&format!("Bearer {api_key}")),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_rejects_an_incorrect_password(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let workspace = tenancy::create_workspace(&pool, tenant.id, "main", None)
        .await
        .unwrap();
    tenancy::create_user(&pool, "someone@example.com", "correct-horse", None)
        .await
        .unwrap();

    let app = build_app(test_state(pool), None);

    let response = rest_request(
        &app,
        "POST",
        "/auth/login",
        None,
        Some(serde_json::json!({
            "email": "someone@example.com",
            "password": "wrong-password",
            "workspace_id": workspace.id,
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_resolves_the_workspace_automatically_when_the_account_has_exactly_one(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let workspace = tenancy::create_workspace(&pool, tenant.id, "main", None)
        .await
        .unwrap();
    let (_invite, token) = tenancy::create_invite(
        &pool,
        tenant.id,
        "member@example.com",
        tenancy::MembershipRole::Member,
        chrono::Duration::hours(1),
    )
    .await
    .unwrap();

    let app = build_app(test_state(pool), None);

    let response = rest_request(
        &app,
        "POST",
        "/auth/signup",
        None,
        Some(serde_json::json!({
            "invite_token": token,
            "password": "hunter2-hunter2",
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    // workspace_id omitted -- the account is only a member of one tenant with one
    // workspace, so it should resolve unambiguously.
    let response = rest_request(
        &app,
        "POST",
        "/auth/login",
        None,
        Some(serde_json::json!({
            "email": "member@example.com",
            "password": "hunter2-hunter2",
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = rest_json_body(response).await;
    assert_eq!(body["workspace_id"], workspace.id.to_string());
}

#[sqlx::test(migrations = "../../migrations")]
#[allow(clippy::await_holding_lock)]
async fn login_requires_workspace_id_when_the_account_has_access_to_more_than_one(pool: PgPool) {
    // Creating a second tenant requires lifting the default single-tenant cap -- see
    // `crate::max_tenants_env_lock` for why this is a shared, crate-wide lock.
    let _guard = crate::max_tenants_env_lock::LOCK.lock().unwrap();
    crate::max_tenants_env_lock::set(Some("0"));

    let tenant_a = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    tenancy::create_workspace(&pool, tenant_a.id, "main", None)
        .await
        .unwrap();
    let tenant_b = tenancy::create_tenant(&pool, "beta", None).await.unwrap();
    tenancy::create_workspace(&pool, tenant_b.id, "main", None)
        .await
        .unwrap();

    let user = tenancy::create_user(&pool, "multi@example.com", "hunter2-hunter2", None)
        .await
        .unwrap();
    tenancy::add_member(&pool, tenant_a.id, user.id, tenancy::MembershipRole::Member)
        .await
        .unwrap();
    tenancy::add_member(&pool, tenant_b.id, user.id, tenancy::MembershipRole::Member)
        .await
        .unwrap();

    let app = build_app(test_state(pool), None);

    let response = rest_request(
        &app,
        "POST",
        "/auth/login",
        None,
        Some(serde_json::json!({
            "email": "multi@example.com",
            "password": "hunter2-hunter2",
        })),
    )
    .await;
    crate::max_tenants_env_lock::set(None);
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_rejects_an_account_with_no_tenant_membership(pool: PgPool) {
    tenancy::create_user(&pool, "orphan@example.com", "hunter2-hunter2", None)
        .await
        .unwrap();

    let app = build_app(test_state(pool), None);

    let response = rest_request(
        &app,
        "POST",
        "/auth/login",
        None,
        Some(serde_json::json!({
            "email": "orphan@example.com",
            "password": "hunter2-hunter2",
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../../migrations")]
async fn auth_endpoints_are_rate_limited_per_caller(pool: PgPool) {
    let app = build_app(test_state(pool), None);

    // The test driver never populates `ConnectInfo`, so every call here falls back to the
    // same shared bucket -- exercising the same "no requester info" path a request behind
    // an unconfigured proxy would take, while still proving the middleware is wired in.
    let mut saw_too_many_requests = false;
    for _ in 0..15 {
        let response = rest_request(
            &app,
            "POST",
            "/auth/login",
            None,
            Some(serde_json::json!({
                "email": "nobody@example.com",
                "password": "wrong",
                "workspace_id": Uuid::nil(),
            })),
        )
        .await;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            saw_too_many_requests = true;
            break;
        }
    }

    assert!(
        saw_too_many_requests,
        "expected /auth/login to start returning 429 after repeated calls from the same caller"
    );
}
