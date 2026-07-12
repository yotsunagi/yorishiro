-- APIキー認証の入口では、まだtenant_idが確定していない（`app.current_tenant`を
-- 設定しようがない）ため、通常のRLS経路ではapi_keysをkey_hashで検索できない。
-- この関数はマイグレーション実行ロール（テーブル所有者）の権限でSECURITY DEFINER
-- 実行し、RLSをこの1関数・1用途だけに限定してバイパスする。key_hash自体は
-- 返さず、認証結果として必要な列（id/tenant_id/scope）のみを返す。
CREATE FUNCTION authenticate_api_key(p_key_hash bytea)
RETURNS TABLE (id uuid, tenant_id uuid, scope text)
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
  SELECT id, tenant_id, scope FROM api_keys WHERE key_hash = p_key_hash
$$;

REVOKE ALL ON FUNCTION authenticate_api_key(bytea) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION authenticate_api_key(bytea) TO yorishiro_app;
