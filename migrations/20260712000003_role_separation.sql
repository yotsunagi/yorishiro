-- テーブル所有者とsuperuserは`FORCE ROW LEVEL SECURITY`があっても常にRLSをバイパスする。
-- そのためRLSを実効させるには、アプリのランタイム接続を所有者(yorishiro)とは別の
-- 非superuser・非owner・NOBYPASSRLSロールで実行する必要がある。yorishiro_appは
-- LOGIN権限を持たない（`SET ROLE`はsuperuserがメンバーシップなしで実行できるため、
-- ログイン自体はyorishiroロールのまま行い、接続確立後に`SET ROLE yorishiro_app`へ
-- 切り替える運用とする）。
-- ロールはクラスタ全体で共有される一方、`sqlx::test`は複数の一時DBへこの
-- マイグレーションを並行適用するため、「存在確認してから作成」ではレースに
-- 負けて重複作成エラーになりうる。CREATE ROLEを直接試み、他の並行実行に
-- 先を越された場合のunique_violationだけを握りつぶす。
DO $$
BEGIN
  CREATE ROLE yorishiro_app NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS NOLOGIN;
EXCEPTION
  WHEN duplicate_object OR unique_violation THEN
    NULL;
END
$$;

ALTER TABLE tenants   FORCE ROW LEVEL SECURITY;
ALTER TABLE api_keys  FORCE ROW LEVEL SECURITY;
ALTER TABLE schemas   FORCE ROW LEVEL SECURITY;
ALTER TABLE entities  FORCE ROW LEVEL SECURITY;
ALTER TABLE relations FORCE ROW LEVEL SECURITY;

GRANT USAGE ON SCHEMA public TO yorishiro_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO yorishiro_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO yorishiro_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO yorishiro_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT USAGE, SELECT ON SEQUENCES TO yorishiro_app;
