use axum::http::StatusCode;
use sqlx::PgPool;
use yorishiro_core::repositories::tenancy;

use super::helpers::*;
use crate::*;

#[sqlx::test(migrations = "../../migrations")]
async fn owner_can_create_list_and_delete_workspaces(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let main = tenancy::create_workspace(&pool, tenant.id, "main", None)
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
        main.id,
        owner.id,
        tenancy::MembershipRole::Owner,
    )
    .await;

    let app = build_app(test_state(pool), None);

    let response = rest_request(
        &app,
        "POST",
        "/api/workspaces",
        Some(&format!("Bearer {owner_key}")),
        Some(serde_json::json!({ "name": "staging" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = rest_json_body(response).await;
    assert_eq!(body["name"], "staging");
    assert_eq!(body["tenant_id"], tenant.id.to_string());
    let staging_id = body["id"].as_str().unwrap().to_string();

    let response = rest_request(
        &app,
        "GET",
        "/api/workspaces",
        Some(&format!("Bearer {owner_key}")),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = rest_json_body(response).await;
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"main"));
    assert!(names.contains(&"staging"));

    let response = rest_request(
        &app,
        "GET",
        &format!("/api/workspaces/{staging_id}"),
        Some(&format!("Bearer {owner_key}")),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = rest_json_body(response).await;
    assert_eq!(body["entity_count"], 0);
    assert_eq!(body["relation_count"], 0);
    assert_eq!(body["schema_count"], 0);

    let response = rest_request(
        &app,
        "DELETE",
        &format!("/api/workspaces/{staging_id}"),
        Some(&format!("Bearer {owner_key}")),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = rest_request(
        &app,
        "GET",
        &format!("/api/workspaces/{staging_id}"),
        Some(&format!("Bearer {owner_key}")),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn cannot_delete_a_tenants_only_workspace(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let main = tenancy::create_workspace(&pool, tenant.id, "main", None)
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
        main.id,
        owner.id,
        tenancy::MembershipRole::Owner,
    )
    .await;

    let app = build_app(test_state(pool), None);

    let response = rest_request(
        &app,
        "DELETE",
        &format!("/api/workspaces/{}", main.id),
        Some(&format!("Bearer {owner_key}")),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[sqlx::test(migrations = "../../migrations")]
async fn member_role_cannot_create_or_delete_workspaces(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let main = tenancy::create_workspace(&pool, tenant.id, "main", None)
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
        main.id,
        member.id,
        tenancy::MembershipRole::Member,
    )
    .await;

    let app = build_app(test_state(pool), None);

    let response = rest_request(
        &app,
        "POST",
        "/api/workspaces",
        Some(&format!("Bearer {member_key}")),
        Some(serde_json::json!({ "name": "staging" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = rest_request(
        &app,
        "DELETE",
        &format!("/api/workspaces/{}", main.id),
        Some(&format!("Bearer {member_key}")),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // A Member-role key can still list/view workspaces, just not create/delete them.
    let response = rest_request(
        &app,
        "GET",
        "/api/workspaces",
        Some(&format!("Bearer {member_key}")),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn workspaces_endpoints_require_authentication(pool: PgPool) {
    let app = build_app(test_state(pool), None);

    let response = rest_request(&app, "GET", "/api/workspaces", None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
#[allow(clippy::await_holding_lock)]
async fn workspace_endpoints_enforce_tenant_isolation(pool: PgPool) {
    // Creating a second tenant requires lifting the default single-tenant cap -- see
    // `crate::max_tenants_env_lock` for why this is a shared, crate-wide lock.
    let _guard = crate::max_tenants_env_lock::LOCK.lock().unwrap();
    crate::max_tenants_env_lock::set(Some("0"));

    let tenant_a = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let workspace_a = tenancy::create_workspace(&pool, tenant_a.id, "main", None)
        .await
        .unwrap();
    let owner_a = tenancy::create_user(&pool, "owner-a@example.com", "hunter2-hunter2", None)
        .await
        .unwrap();
    tenancy::add_member(
        &pool,
        tenant_a.id,
        owner_a.id,
        tenancy::MembershipRole::Owner,
    )
    .await
    .unwrap();
    let owner_a_key = issue_key_for(
        &pool,
        tenant_a.id,
        workspace_a.id,
        owner_a.id,
        tenancy::MembershipRole::Owner,
    )
    .await;

    let tenant_b = tenancy::create_tenant(&pool, "beta", None).await.unwrap();
    let workspace_b = tenancy::create_workspace(&pool, tenant_b.id, "main", None)
        .await
        .unwrap();
    crate::max_tenants_env_lock::set(None);

    let app = build_app(test_state(pool), None);

    let response = rest_request(
        &app,
        "GET",
        &format!("/api/workspaces/{}", workspace_b.id),
        Some(&format!("Bearer {owner_a_key}")),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = rest_request(
        &app,
        "DELETE",
        &format!("/api/workspaces/{}", workspace_b.id),
        Some(&format!("Bearer {owner_a_key}")),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
