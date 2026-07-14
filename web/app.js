// Yorishiro admin dashboard -- a deliberately framework-free SPA. Scope is limited to first-run
// setup, login, usage/billing display, and member management; it is not a general
// entity/schema/relation browser -- that's what the REST API + Swagger UI (`/docs` on
// yorishiro-server) are for. Served by both yorishiro-server (via `YSR_WEB_DIR`) and
// yorishiro-hosted-server -- `/hosted/tenant/overview` and friends only exist on the latter, so
// on yorishiro-server alone, `#/dashboard` degrades to just showing the API key login just
// issued (see `renderLoginComplete`) instead of the full dashboard.

const SESSION_KEY = "yorishiro_session";

function apiBase() {
  return (window.YORISHIRO_CONFIG && window.YORISHIRO_CONFIG.apiBase) || "";
}

function getSession() {
  const raw = sessionStorage.getItem(SESSION_KEY);
  return raw ? JSON.parse(raw) : null;
}

function setSession(session) {
  sessionStorage.setItem(SESSION_KEY, JSON.stringify(session));
}

function clearSession() {
  sessionStorage.removeItem(SESSION_KEY);
}

function el(html) {
  const template = document.createElement("template");
  template.innerHTML = html.trim();
  return template.content.firstElementChild;
}

function mount(node) {
  const app = document.getElementById("app");
  app.replaceChildren(node);
}

async function parseErrorMessage(response) {
  try {
    const body = await response.json();
    return body?.error?.message || `request failed (${response.status})`;
  } catch {
    return `request failed (${response.status})`;
  }
}

async function checkSetupStatus() {
  try {
    const response = await fetch(`${apiBase()}/setup/status`);
    if (!response.ok) {
      return { setup_required: false };
    }
    return response.json();
  } catch {
    return { setup_required: false };
  }
}

async function setup({ email, password, displayName }) {
  const response = await fetch(`${apiBase()}/setup`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ email, password, display_name: displayName || undefined }),
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function login({ email, password, workspaceId }) {
  const response = await fetch(`${apiBase()}/auth/login`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ email, password, workspace_id: workspaceId }),
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function fetchTenantOverview(apiKey) {
  const response = await fetch("/hosted/tenant/overview", {
    headers: { authorization: `Bearer ${apiKey}` },
  });
  if (!response.ok) {
    const error = new Error(await parseErrorMessage(response));
    error.status = response.status;
    throw error;
  }
  return response.json();
}

async function addMember(apiKey, { email, role }) {
  const response = await fetch(`${apiBase()}/api/members`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify({ email, role }),
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

function renderSetup(errorMessage) {
  const view = el(`
    <div>
      <h1>Welcome to Yorishiro</h1>
      <p class="hint">This deployment has no tenant yet. Create the owner account to get started.</p>
      <form id="setup-form">
        <label>Email
          <input type="email" name="email" required autocomplete="username">
        </label>
        <label>Password
          <input type="password" name="password" required autocomplete="new-password" minlength="8">
        </label>
        <label>Display name (optional)
          <input type="text" name="displayName" autocomplete="name">
        </label>
        ${errorMessage ? `<p class="error">${errorMessage}</p>` : ""}
        <button type="submit">Create owner account</button>
      </form>
    </div>
  `);

  view.querySelector("#setup-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = new FormData(event.target);
    try {
      const result = await setup({
        email: form.get("email"),
        password: form.get("password"),
        displayName: form.get("displayName"),
      });
      mount(renderSetupComplete(result));
    } catch (err) {
      mount(renderSetup(err.message));
    }
  });

  return view;
}

function renderSetupComplete(result) {
  return el(`
    <div>
      <h1>Setup complete</h1>
      <p class="hint">The tenant, workspace, and owner account have been created.</p>
      <dl>
        <dt>Email</dt><dd>${result.email}</dd>
        <dt>Workspace ID</dt><dd><code>${result.workspace_id}</code></dd>
      </dl>
      <p class="error"><strong>Save this API key now -- it is only ever shown once:</strong></p>
      <pre>${result.api_key}</pre>
      <p class="hint">Use it as a Bearer token against the REST API (see <a href="/docs">/docs</a>) or your MCP client's configuration.</p>
      <p><a href="#/login">Continue to sign in</a></p>
    </div>
  `);
}

function renderLogin(errorMessage) {
  const view = el(`
    <div>
      <h1>Yorishiro</h1>
      <p class="hint">Sign in with the account created via setup or an invite (see /auth/signup).</p>
      <form id="login-form">
        <label>Email
          <input type="email" name="email" required autocomplete="username">
        </label>
        <label>Password
          <input type="password" name="password" required autocomplete="current-password">
        </label>
        <label>Workspace ID
          <input type="text" name="workspaceId" required placeholder="00000000-0000-0000-0000-000000000000">
        </label>
        <p class="hint">Ask a tenant owner for your workspace's id, or find it in your signup response.</p>
        ${errorMessage ? `<p class="error">${errorMessage}</p>` : ""}
        <button type="submit">Sign in</button>
      </form>
    </div>
  `);

  view.querySelector("#login-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = new FormData(event.target);
    try {
      const result = await login({
        email: form.get("email"),
        password: form.get("password"),
        workspaceId: form.get("workspaceId"),
      });
      setSession({ apiKey: result.api_key, email: result.email ?? form.get("email") });
      location.hash = "#/dashboard";
    } catch (err) {
      mount(renderLogin(err.message));
    }
  });

  return view;
}

function renderLoginComplete(session) {
  const view = el(`
    <div>
      <div class="top-bar">
        <h1>Signed in</h1>
        <button class="secondary" id="logout-button">Sign out</button>
      </div>
      <p class="hint">This is the community edition, which has no dashboard here -- use the API key
      below as a Bearer token against the REST API (see <a href="/docs">/docs</a>) or your MCP
      client's configuration.</p>
      <dl>
        <dt>Email</dt><dd>${session.email}</dd>
      </dl>
      <pre>${session.apiKey}</pre>
    </div>
  `);

  view.querySelector("#logout-button").addEventListener("click", () => {
    clearSession();
    location.hash = "#/login";
  });

  return view;
}

function renderMembersTable(members) {
  const rows = members
    .map(
      (member) => `
        <tr>
          <td>${member.email}</td>
          <td>${member.display_name ?? ""}</td>
          <td>${member.role}</td>
        </tr>`,
    )
    .join("");
  return `
    <table>
      <thead><tr><th>Email</th><th>Name</th><th>Role</th></tr></thead>
      <tbody>${rows}</tbody>
    </table>
  `;
}

function renderDashboardShell(overview, addMemberError) {
  const view = el(`
    <div>
      <div class="top-bar">
        <h1>Tenant Dashboard</h1>
        <button class="secondary" id="logout-button">Sign out</button>
      </div>
      <p class="hint">Tenant ${overview.tenant_id} &middot; plan: ${overview.plan ?? "self-hosted / unmetered"}</p>

      <div class="stat-grid">
        <div class="stat"><div class="value">${overview.usage.workspace_count}</div><div class="label">workspaces${overview.max_workspaces != null ? ` / ${overview.max_workspaces}` : ""}</div></div>
        <div class="stat"><div class="value">${overview.usage.member_count}</div><div class="label">members</div></div>
        <div class="stat"><div class="value">${overview.usage.entity_count}</div><div class="label">entities</div></div>
      </div>

      <h2>Members</h2>
      ${renderMembersTable(overview.members)}

      <h2>Add a member</h2>
      <p class="hint">The person must already have an account (created via /auth/signup from an invite).</p>
      <form id="add-member-form">
        <label>Email
          <input type="email" name="email" required>
        </label>
        <label>Role
          <select name="role">
            <option value="viewer">Viewer</option>
            <option value="member" selected>Member</option>
            <option value="admin">Admin</option>
            <option value="owner">Owner</option>
          </select>
        </label>
        ${addMemberError ? `<p class="error">${addMemberError}</p>` : ""}
        <button type="submit">Add member</button>
      </form>
    </div>
  `);

  view.querySelector("#logout-button").addEventListener("click", () => {
    clearSession();
    location.hash = "#/login";
  });

  view.querySelector("#add-member-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const session = getSession();
    const form = new FormData(event.target);
    try {
      await addMember(session.apiKey, {
        email: form.get("email"),
        role: form.get("role"),
      });
      await renderDashboard();
    } catch (err) {
      mount(renderDashboardShell(overview, err.message));
    }
  });

  return view;
}

async function renderDashboard() {
  const session = getSession();
  if (!session) {
    location.hash = "#/login";
    return;
  }

  mount(el(`<p>Loading…</p>`));
  try {
    const overview = await fetchTenantOverview(session.apiKey);
    mount(renderDashboardShell(overview));
  } catch (err) {
    if (err.status === 404) {
      // Not a session failure -- this deployment is yorishiro-server (community edition),
      // which has no /hosted/tenant/overview endpoint. The session (and the API key login
      // just issued) is still valid.
      mount(renderLoginComplete(session));
      return;
    }
    clearSession();
    mount(renderLogin(`session expired: ${err.message}`));
  }
}

async function router() {
  const hash = location.hash || "#/login";
  if (hash === "#/dashboard") {
    renderDashboard();
    return;
  }

  const status = await checkSetupStatus();
  if (status.setup_required && hash !== "#/setup") {
    location.hash = "#/setup";
    return;
  }
  if (!status.setup_required && hash === "#/setup") {
    location.hash = "#/login";
    return;
  }

  mount(hash === "#/setup" ? renderSetup() : renderLogin());
}

window.addEventListener("hashchange", router);
window.addEventListener("DOMContentLoaded", router);
