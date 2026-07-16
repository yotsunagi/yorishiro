use axum::http::StatusCode;
use sqlx::PgPool;
use yorishiro_core::repositories::tenancy;

use super::helpers::*;
use crate::*;

#[sqlx::test(migrations = "../../migrations")]
async fn owner_can_list_and_add_members(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let workspace = tenancy::create_workspace(&pool, tenant.id, "main", None)
        .await
        .unwrap();
    let owner = tenancy::create_user(&pool, "owner@example.com", "hunter2-hunter2", None)
        .await
        .unwrap();
    tenancy::add_member(&pool, tenant.id, owner.id, tenancy::MembershipRole::Owner)
        .await
        .unwrap();
    let owner_key = issue_key_for(
        &pool,
        tenant.id,
        workspace.id,
        owner.id,
        tenancy::MembershipRole::Owner,
    )
    .await;

    // The invitee must already have an account before they can be added by email.
    let invitee = tenancy::create_user(&pool, "invitee@example.com", "hunter2-hunter2", None)
        .await
        .unwrap();

    let app = build_app(test_state(pool.clone()), None);

    let response = rest_request(
        &app,
        "POST",
        "/api/members",
        Some(&format!("Bearer {owner_key}")),
        Some(serde_json::json!({
            "email": "invitee@example.com",
            "role": "member",
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = rest_json_body(response).await;
    assert_eq!(body["user_id"], invitee.id.to_string());
    assert_eq!(body["role"], "member");

    let response = rest_request(
        &app,
        "GET",
        "/api/members",
        Some(&format!("Bearer {owner_key}")),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = rest_json_body(response).await;
    let emails: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["email"].as_str().unwrap())
        .collect();
    assert!(emails.contains(&"owner@example.com"));
    assert!(emails.contains(&"invitee@example.com"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn add_member_rejects_an_email_with_no_account(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let workspace = tenancy::create_workspace(&pool, tenant.id, "main", None)
        .await
        .unwrap();
    let owner = tenancy::create_user(&pool, "owner@example.com", "hunter2-hunter2", None)
        .await
        .unwrap();
    tenancy::add_member(&pool, tenant.id, owner.id, tenancy::MembershipRole::Owner)
        .await
        .unwrap();
    let owner_key = issue_key_for(
        &pool,
        tenant.id,
        workspace.id,
        owner.id,
        tenancy::MembershipRole::Owner,
    )
    .await;

    let app = build_app(test_state(pool), None);

    let response = rest_request(
        &app,
        "POST",
        "/api/members",
        Some(&format!("Bearer {owner_key}")),
        Some(serde_json::json!({
            "email": "nobody@example.com",
            "role": "member",
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn member_role_cannot_manage_members(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let workspace = tenancy::create_workspace(&pool, tenant.id, "main", None)
        .await
        .unwrap();
    let member = tenancy::create_user(&pool, "member@example.com", "hunter2-hunter2", None)
        .await
        .unwrap();
    tenancy::add_member(&pool, tenant.id, member.id, tenancy::MembershipRole::Member)
        .await
        .unwrap();
    let member_key = issue_key_for(
        &pool,
        tenant.id,
        workspace.id,
        member.id,
        tenancy::MembershipRole::Member,
    )
    .await;

    let app = build_app(test_state(pool), None);

    let response = rest_request(
        &app,
        "GET",
        "/api/members",
        Some(&format!("Bearer {member_key}")),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../../migrations")]
async fn members_endpoints_require_authentication(pool: PgPool) {
    let app = build_app(test_state(pool), None);

    let response = rest_request(&app, "GET", "/api/members", None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
